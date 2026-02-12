import bleach
import markdown as md

_ALLOWED_TAGS = [
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "code",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "strong",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "ul",
]

_ALLOWED_ATTRIBUTES = {
    "a": ["href", "title", "rel"],
    "abbr": ["title"],
    "code": ["class"],
    "th": ["colspan", "rowspan"],
    "td": ["colspan", "rowspan"],
}


def render_markdown(text: str) -> str:
    raw_html = md.markdown(
        text or "",
        extensions=[
            "fenced_code",
            "tables",
            "sane_lists",
            "nl2br",
        ],
    )
    cleaned = bleach.clean(
        raw_html,
        tags=_ALLOWED_TAGS,
        attributes=_ALLOWED_ATTRIBUTES,
        protocols=["http", "https", "mailto"],
        strip=True,
    )
    return bleach.linkify(cleaned)

