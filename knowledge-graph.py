#!/usr/bin/env python3
"""
AetherVault Knowledge Graph Engine
A lightweight personal knowledge graph using SQLite + NetworkX.
"""

import argparse
import json
import os
import re
import sys
import fcntl
import time
from contextlib import contextmanager
from datetime import datetime, timezone

import networkx as nx
from networkx.readwrite import json_graph

# Use AETHERVAULT_HOME env var, defaulting to ~/.aethervault
AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
DEFAULT_GRAPH_FILE = os.path.join(AETHERVAULT_HOME, "data", "knowledge-graph.json")
DEFAULT_CONFIG_FILE = os.path.join(AETHERVAULT_HOME, "config", "knowledge-graph.json")


def load_config():
    if os.path.exists(DEFAULT_CONFIG_FILE):
        with open(DEFAULT_CONFIG_FILE, "r") as f:
            return json.load(f)
    return {
        "entity_types": ["person", "project", "technology", "organization",
                         "preference", "topic", "location"],
        "relation_types": ["owns", "works-on", "uses", "runs-on", "part-of",
                           "knows", "prefers", "located-at"],
        "auto_ingest": False,
        "graph_file": DEFAULT_GRAPH_FILE,
    }


def get_graph_file():
    config = load_config()
    return config.get("graph_file", DEFAULT_GRAPH_FILE)


def load_graph(graph_file=None):
    """Load graph with shared lock (for read-only operations)."""
    graph_file = graph_file or get_graph_file()
    if os.path.exists(graph_file):
        lock_path = graph_file + ".lock"
        os.makedirs(os.path.dirname(lock_path), exist_ok=True)
        with open(lock_path, "w") as lock_fd:
            fcntl.flock(lock_fd, fcntl.LOCK_SH)
            try:
                with open(graph_file, "r") as f:
                    data = json.load(f)
                G = json_graph.node_link_graph(data)
                return G
            finally:
                fcntl.flock(lock_fd, fcntl.LOCK_UN)
    return nx.DiGraph()


def save_graph(G, graph_file=None):
    """Save graph with exclusive lock (standalone, for backward compat)."""
    graph_file = graph_file or get_graph_file()
    os.makedirs(os.path.dirname(graph_file), exist_ok=True)
    lock_path = graph_file + ".lock"
    with open(lock_path, "w") as lock_fd:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        try:
            _save_graph_unlocked(G, graph_file)
        finally:
            fcntl.flock(lock_fd, fcntl.LOCK_UN)


def _save_graph_unlocked(G, graph_file):
    """Save graph to disk (caller must hold exclusive lock)."""
    data = json_graph.node_link_data(G)
    tmp_path = graph_file + ".tmp"
    with open(tmp_path, "w") as f:
        json.dump(data, f, indent=2, default=str)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp_path, graph_file)


@contextmanager
def graph_transaction(graph_file=None):
    """Atomic read-modify-write: holds exclusive lock across load + save.

    Usage:
        with graph_transaction() as G:
            add_entity(G, "person", "Alice")
            # G is saved automatically on clean exit
    """
    graph_file = graph_file or get_graph_file()
    os.makedirs(os.path.dirname(graph_file), exist_ok=True)
    lock_path = graph_file + ".lock"
    with open(lock_path, "w") as lock_fd:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        try:
            if os.path.exists(graph_file):
                with open(graph_file, "r") as f:
                    data = json.load(f)
                G = json_graph.node_link_graph(data)
            else:
                G = nx.DiGraph()
            yield G
            _save_graph_unlocked(G, graph_file)
        finally:
            fcntl.flock(lock_fd, fcntl.LOCK_UN)


def now_iso():
    return datetime.now(timezone.utc).isoformat()


def normalize_name(name):
    """Normalize entity name for matching (case-insensitive lookup key)."""
    return name.strip()


def find_node(G, name):
    """Find a node by name, case-insensitive."""
    name_lower = name.strip().lower()
    for node_id, attrs in G.nodes(data=True):
        if attrs.get("name", "").lower() == name_lower:
            return node_id
    return None


def add_entity(G, entity_type, name, attrs=None):
    name = normalize_name(name)
    existing = find_node(G, name)
    if existing is not None:
        # Update existing entity -- don't downgrade from a specific type to "topic"
        current_type = G.nodes[existing].get("type", "topic")
        if entity_type != "topic" or current_type == "topic":
            G.nodes[existing]["type"] = entity_type
        G.nodes[existing]["updated_at"] = now_iso()
        if attrs:
            props = G.nodes[existing].get("properties", {})
            props.update(attrs)
            G.nodes[existing]["properties"] = props
        return existing, False
    else:
        node_id = name
        G.add_node(node_id,
                    type=entity_type,
                    name=name,
                    properties=attrs or {},
                    created_at=now_iso(),
                    updated_at=now_iso())
        return node_id, True


def add_relation(G, from_name, relation, to_name, confidence=1.0, source="manual"):
    from_id = find_node(G, from_name)
    to_id = find_node(G, to_name)
    if from_id is None:
        print(f"Warning: Entity '{from_name}' not found. Creating as 'topic' type.")
        from_id, _ = add_entity(G, "topic", from_name)
    if to_id is None:
        print(f"Warning: Entity '{to_name}' not found. Creating as 'topic' type.")
        to_id, _ = add_entity(G, "topic", to_name)
    G.add_edge(from_id, to_id,
               relation=relation,
               confidence=confidence,
               source=source,
               created_at=now_iso())
    return from_id, to_id


def query_by_name(G, name_query):
    results = []
    query_lower = name_query.lower()
    for node_id, attrs in G.nodes(data=True):
        if query_lower in attrs.get("name", "").lower():
            results.append((node_id, attrs))
    return results


def query_by_type(G, entity_type):
    results = []
    for node_id, attrs in G.nodes(data=True):
        if attrs.get("type", "").lower() == entity_type.lower():
            results.append((node_id, attrs))
    return results


def query_related_to(G, name):
    node_id = find_node(G, name)
    if node_id is None:
        return []
    results = []
    # Outgoing edges
    for _, target, edge_attrs in G.out_edges(node_id, data=True):
        target_attrs = G.nodes[target]
        results.append({
            "direction": "outgoing",
            "relation": edge_attrs.get("relation", "related"),
            "entity": target_attrs.get("name", target),
            "entity_type": target_attrs.get("type", "unknown"),
        })
    # Incoming edges
    for source, _, edge_attrs in G.in_edges(node_id, data=True):
        source_attrs = G.nodes[source]
        results.append({
            "direction": "incoming",
            "relation": edge_attrs.get("relation", "related"),
            "entity": source_attrs.get("name", source),
            "entity_type": source_attrs.get("type", "unknown"),
        })
    return results


# --- NLP Ingestion ---

# Entity name pattern: capitalized word(s), allowing dots in version numbers (e.g. "Claude Opus 4.6")
# Only continues to the next word if it starts with uppercase or is a number (version)
_ENT = r"([A-Z][a-zA-Z0-9]*(?:(?:\.[0-9][a-zA-Z0-9.]*)|(?:\s+[A-Z][a-zA-Z0-9]*)|(?:\s+[0-9][a-zA-Z0-9.]*))*)"

# Patterns for entity/relation extraction
RELATION_PATTERNS = [
    # "X is working on Y" / "X works on Y"
    (re.compile(_ENT + r"\s+(?:is\s+)?work(?:s|ing)\s+on\s+" + _ENT), "works-on"),
    # "X owns Y"
    (re.compile(_ENT + r"\s+owns?\s+" + _ENT), "owns"),
    # "X uses Y"
    (re.compile(_ENT + r"\s+uses?\s+" + _ENT), "uses"),
    # "X runs on Y" / "X runs Y"
    (re.compile(_ENT + r"\s+runs?\s+(?:on\s+)?" + _ENT), "runs-on"),
    # "X is part of Y"
    (re.compile(_ENT + r"\s+is\s+part\s+of\s+" + _ENT), "part-of"),
    # "X knows Y"
    (re.compile(_ENT + r"\s+knows?\s+" + _ENT), "knows"),
    # "X prefers Y"
    (re.compile(_ENT + r"\s+prefers?\s+" + _ENT), "prefers"),
    # "X is located at/in Y"
    (re.compile(_ENT + r"\s+is\s+located\s+(?:at|in)\s+" + _ENT), "located-at"),
    # "X has Y" (generic)
    (re.compile(_ENT + r"\s+has\s+" + _ENT), "has"),
]

ENTITY_PATTERNS = [
    # "X is a/the Y" -> X is entity of type inferred from Y
    (re.compile(_ENT + r"\s+is\s+(?:a|an|the)\s+(\w+)"),),
]

# Words to skip as entities
STOP_WORDS = {
    "The", "This", "That", "These", "Those", "He", "She", "It", "They",
    "His", "Her", "Its", "Their", "We", "You", "Who", "What", "Which",
    "There", "Here", "And", "But", "Or", "Not", "So", "If", "Then",
}

TYPE_HINTS = {
    "person": ["person", "developer", "engineer", "owner", "founder", "user",
               "creator", "admin", "manager", "assistant"],
    "project": ["project", "app", "application", "tool", "system", "bot",
                "service", "platform", "assistant"],
    "technology": ["technology", "model", "llm", "ai", "framework", "library",
                   "language", "database", "engine", "subagent", "agent"],
    "organization": ["company", "organization", "org", "provider", "cloud",
                     "team", "group"],
    "location": ["city", "country", "region", "server", "datacenter", "droplet"],
}


def infer_type(descriptor):
    descriptor_lower = descriptor.lower()
    for etype, keywords in TYPE_HINTS.items():
        if descriptor_lower in keywords:
            return etype
    return "topic"


def extract_proper_nouns(text):
    """Extract potential entity names (capitalized words/phrases)."""
    # Split on sentence boundaries first, then extract within each sentence
    sentences = re.split(r'[.!?]\s+', text)
    entities = set()
    for sentence in sentences:
        # Match capitalized word sequences (allowing version numbers like 4.6)
        pattern = r"([A-Z][a-zA-Z0-9]*(?:[ ][A-Z][a-zA-Z0-9]*)*(?:[ ]\d+[a-zA-Z0-9.]*)*)"
        matches = re.findall(pattern, sentence)
        for m in matches:
            m = m.strip().rstrip(".")
            if m not in STOP_WORDS and len(m) > 1:
                entities.add(m)
    return entities


def ingest_text(G, text):
    """Extract entities and relations from text using regex patterns."""
    added_entities = []
    added_relations = []

    # Extract relations
    for compiled_re, rel_type in RELATION_PATTERNS:
        for match in compiled_re.finditer(text):
            subj = match.group(1).strip()
            obj = match.group(2).strip()
            if subj in STOP_WORDS or obj in STOP_WORDS:
                continue
            # Ensure entities exist
            sid, new_s = add_entity(G, "topic", subj)
            if new_s:
                added_entities.append(subj)
            oid, new_o = add_entity(G, "topic", obj)
            if new_o:
                added_entities.append(obj)
            add_relation(G, subj, rel_type, obj, confidence=0.8, source="ingest")
            added_relations.append((subj, rel_type, obj))

    # Extract "X is a Y" patterns for entity typing
    for (compiled_re,) in ENTITY_PATTERNS:
        for match in compiled_re.finditer(text):
            entity_name = match.group(1).strip()
            descriptor = match.group(2).strip()
            if entity_name in STOP_WORDS:
                continue
            etype = infer_type(descriptor)
            eid, new_e = add_entity(G, etype, entity_name)
            if new_e:
                added_entities.append(entity_name)
            else:
                # Update type if we got a better inference
                if etype != "topic":
                    G.nodes[eid]["type"] = etype
                    G.nodes[eid]["updated_at"] = now_iso()

    # Extract remaining proper nouns as potential entities
    proper_nouns = extract_proper_nouns(text)
    for noun in proper_nouns:
        if find_node(G, noun) is None:
            _, new = add_entity(G, "topic", noun)
            if new:
                added_entities.append(noun)

    return added_entities, added_relations


def get_summary(G, topic):
    """Generate a context summary for a topic by traversing the graph."""
    node_id = find_node(G, topic)
    if node_id is None:
        return None

    attrs = G.nodes[node_id]
    lines = []
    lines.append(f"=== {attrs.get('name', node_id)} ===")
    lines.append(f"Type: {attrs.get('type', 'unknown')}")
    props = attrs.get("properties", {})
    if props:
        for k, v in props.items():
            lines.append(f"  {k}: {v}")
    lines.append("")

    # Outgoing relations
    out_edges = list(G.out_edges(node_id, data=True))
    if out_edges:
        lines.append("Relationships (outgoing):")
        for _, target, eattrs in out_edges:
            tname = G.nodes[target].get("name", target)
            ttype = G.nodes[target].get("type", "unknown")
            rel = eattrs.get("relation", "related")
            lines.append(f"  -> {rel} -> {tname} ({ttype})")

    # Incoming relations
    in_edges = list(G.in_edges(node_id, data=True))
    if in_edges:
        lines.append("Relationships (incoming):")
        for source, _, eattrs in in_edges:
            sname = G.nodes[source].get("name", source)
            stype = G.nodes[source].get("type", "unknown")
            rel = eattrs.get("relation", "related")
            lines.append(f"  <- {rel} <- {sname} ({stype})")

    # Second-degree connections
    neighbors = set()
    for _, target, _ in out_edges:
        for _, t2, e2 in G.out_edges(target, data=True):
            if t2 != node_id:
                neighbors.add((G.nodes[target].get("name", target),
                               e2.get("relation", "related"),
                               G.nodes[t2].get("name", t2)))
    for source, _, _ in in_edges:
        for s2, _, e2 in G.in_edges(source, data=True):
            if s2 != node_id:
                neighbors.add((G.nodes[s2].get("name", s2),
                               e2.get("relation", "related"),
                               G.nodes[source].get("name", source)))

    if neighbors:
        lines.append("")
        lines.append("Extended network:")
        for subj, rel, obj in neighbors:
            lines.append(f"  {subj} -> {rel} -> {obj}")

    return "\n".join(lines)


def format_entity(node_id, attrs):
    name = attrs.get("name", node_id)
    etype = attrs.get("type", "unknown")
    props = attrs.get("properties", {})
    parts = [f"{name} [{etype}]"]
    if props:
        prop_str = ", ".join(f"{k}={v}" for k, v in props.items())
        parts.append(f"  Properties: {prop_str}")
    return "\n".join(parts)


# --- CLI ---

def cmd_add_entity(args):
    attrs = {}
    if args.attrs:
        attrs = json.loads(args.attrs)
    with graph_transaction() as G:
        node_id, is_new = add_entity(G, args.type, args.name, attrs)
    if is_new:
        print(f"Added entity: {args.name} [{args.type}]")
    else:
        print(f"Updated entity: {args.name} [{args.type}]")


def cmd_add_relation(args):
    with graph_transaction() as G:
        from_id, to_id = add_relation(G, args.from_entity, args.relation, args.to_entity,
                                       confidence=args.confidence, source="manual")
    print(f"Added relation: {args.from_entity} --[{args.relation}]--> {args.to_entity}")


def cmd_query(args):
    G = load_graph()
    if args.name:
        results = query_by_name(G, args.name)
        if not results:
            print(f"No entities matching '{args.name}'")
            return
        print(f"Entities matching '{args.name}':")
        for node_id, attrs in results:
            print(f"  {format_entity(node_id, attrs)}")
    elif args.type:
        results = query_by_type(G, args.type)
        if not results:
            print(f"No entities of type '{args.type}'")
            return
        print(f"Entities of type '{args.type}':")
        for node_id, attrs in results:
            print(f"  {format_entity(node_id, attrs)}")
    elif args.related_to:
        results = query_related_to(G, args.related_to)
        if not results:
            print(f"No relations found for '{args.related_to}'")
            return
        print(f"Relations for '{args.related_to}':")
        for r in results:
            if r["direction"] == "outgoing":
                print(f"  -> {r['relation']} -> {r['entity']} ({r['entity_type']})")
            else:
                print(f"  <- {r['relation']} <- {r['entity']} ({r['entity_type']})")
    else:
        print("Specify --name, --type, or --related-to")


def cmd_ingest(args):
    with graph_transaction() as G:
        entities, relations = ingest_text(G, args.text)
    print(f"Ingested: {len(entities)} new entities, {len(relations)} relations")
    if entities:
        print(f"  Entities: {', '.join(entities)}")
    if relations:
        for s, r, o in relations:
            print(f"  Relation: {s} --[{r}]--> {o}")


def cmd_summary(args):
    G = load_graph()
    summary = get_summary(G, args.topic)
    if summary is None:
        print(f"Topic '{args.topic}' not found in graph")
        sys.exit(1)
    print(summary)


def cmd_list(args):
    G = load_graph()
    nodes = list(G.nodes(data=True))
    if not nodes:
        print("Graph is empty")
        return
    print(f"Knowledge Graph: {len(nodes)} entities, {G.number_of_edges()} relations\n")
    # Group by type
    by_type = {}
    for node_id, attrs in nodes:
        etype = attrs.get("type", "unknown")
        by_type.setdefault(etype, []).append((node_id, attrs))
    for etype in sorted(by_type.keys()):
        print(f"[{etype}]")
        for node_id, attrs in by_type[etype]:
            print(f"  {format_entity(node_id, attrs)}")
        print()


def cmd_export(args):
    G = load_graph()
    data = json_graph.node_link_data(G)
    print(json.dumps(data, indent=2, default=str))


def main():
    parser = argparse.ArgumentParser(description="AetherVault Knowledge Graph Engine")
    subparsers = parser.add_subparsers(dest="command", help="Command to run")

    # add-entity
    p_add = subparsers.add_parser("add-entity", help="Add or update an entity")
    p_add.add_argument("--type", required=True, help="Entity type")
    p_add.add_argument("--name", required=True, help="Entity name")
    p_add.add_argument("--attrs", default=None, help="JSON attributes")
    p_add.set_defaults(func=cmd_add_entity)

    # add-relation
    p_rel = subparsers.add_parser("add-relation", help="Add a relation between entities")
    p_rel.add_argument("--from", dest="from_entity", required=True, help="Source entity name")
    p_rel.add_argument("--relation", required=True, help="Relation type")
    p_rel.add_argument("--to", dest="to_entity", required=True, help="Target entity name")
    p_rel.add_argument("--confidence", type=float, default=1.0, help="Confidence score (0-1)")
    p_rel.set_defaults(func=cmd_add_relation)

    # query
    p_query = subparsers.add_parser("query", help="Query the knowledge graph")
    p_query.add_argument("--name", default=None, help="Search by name (partial match)")
    p_query.add_argument("--type", default=None, help="Filter by entity type")
    p_query.add_argument("--related-to", default=None, help="Find relations for entity")
    p_query.set_defaults(func=cmd_query)

    # ingest
    p_ingest = subparsers.add_parser("ingest", help="Ingest text to extract entities/relations")
    p_ingest.add_argument("--text", required=True, help="Text to ingest")
    p_ingest.set_defaults(func=cmd_ingest)

    # summary
    p_summary = subparsers.add_parser("summary", help="Get context summary for a topic")
    p_summary.add_argument("--topic", required=True, help="Topic to summarize")
    p_summary.set_defaults(func=cmd_summary)

    # list
    p_list = subparsers.add_parser("list", help="List all entities")
    p_list.set_defaults(func=cmd_list)

    # export
    p_export = subparsers.add_parser("export", help="Export full graph as JSON")
    p_export.set_defaults(func=cmd_export)

    args = parser.parse_args()
    if args.command is None:
        parser.print_help()
        sys.exit(1)

    args.func(args)


if __name__ == "__main__":
    main()
