import os
import re
import json
import socket
import ipaddress
import urllib.request
import urllib.error
from urllib.parse import urlparse, urlencode

from django.contrib.auth.models import User
from django.contrib import messages
from django.db.models import Q
from django.shortcuts import get_object_or_404, redirect, render
from django.views.decorators.http import require_http_methods

from posts.models import Post

from .forms import ProfileForm
from .models import AgentClaim


_X_HANDLE_RE = re.compile(r"^[A-Za-z0-9_]{1,15}$")
_X_OEMBED_URL = "https://publish.twitter.com/oembed"


def _is_public_ip(ip: str) -> bool:
    try:
        addr = ipaddress.ip_address(ip)
    except ValueError:
        return False
    return bool(getattr(addr, "is_global", False))


def _host_resolves_to_public_ip(host: str, port: int) -> bool:
    try:
        infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except OSError:
        return False
    resolved = {sockaddr[0] for *_, sockaddr in infos if sockaddr}
    if not resolved:
        return False
    return all(_is_public_ip(ip) for ip in resolved)


class _SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parsed = urlparse(newurl)
        host = (parsed.hostname or "").strip()
        if not host:
            return None
        if parsed.scheme not in ("http", "https"):
            return None
        port = parsed.port or (443 if parsed.scheme == "https" else 80)
        if not _host_resolves_to_public_ip(host, port):
            return None
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _extract_x_handle(proof_url: str):
    parsed = urlparse(proof_url)
    host = (parsed.netloc or "").split(":")[0].lower()
    if host.startswith("www."):
        host = host[4:]
    if host.startswith("mobile."):
        host = host[7:]
    if host not in ("x.com", "twitter.com"):
        return None

    parts = [p for p in (parsed.path or "").split("/") if p]
    if len(parts) >= 3 and parts[1] == "status":
        handle = parts[0].lstrip("@")
        if _X_HANDLE_RE.match(handle):
            return handle
    return ""


def _looks_like_x_status_url(proof_url: str) -> bool:
    parsed = urlparse(proof_url)
    host = (parsed.netloc or "").split(":")[0].lower()
    if host.startswith("www."):
        host = host[4:]
    if host.startswith("mobile."):
        host = host[7:]
    if host not in ("x.com", "twitter.com"):
        return False
    parts = [p for p in (parsed.path or "").split("/") if p]
    for idx, part in enumerate(parts):
        if part == "status" and idx + 1 < len(parts) and parts[idx + 1].isdigit():
            return True
    return False


def _handle_from_oembed_author_url(author_url: str) -> str:
    parsed = urlparse(author_url or "")
    parts = [p for p in (parsed.path or "").split("/") if p]
    if not parts:
        return ""
    handle = parts[-1].lstrip("@")
    if _X_HANDLE_RE.match(handle):
        return handle
    return ""


def _fetch_x_oembed(proof_url: str) -> dict:
    url = _X_OEMBED_URL + "?" + urlencode({"omit_script": "1", "url": proof_url})
    req = urllib.request.Request(
        url, headers={"User-Agent": "tachyongrid-claim/0.2"}
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        body = resp.read(250_000)
    data = json.loads(body.decode("utf-8", errors="ignore") or "{}")
    if not isinstance(data, dict):
        return {}
    data["_raw"] = body.decode("utf-8", errors="ignore")
    return data


def profile(request, username: str):
    user = get_object_or_404(User, username=username)
    posts = Post.objects.filter(author=user, is_removed=False).select_related(
        "community", "topic"
    )
    if request.user.is_authenticated:
        posts = posts.filter(
            Q(community__is_private=False)
            | Q(community__memberships__user=request.user)
        ).distinct()
    else:
        posts = posts.filter(community__is_private=False)
    posts = posts.order_by("-created_at")[:25]
    return render(
        request, "accounts/profile.html", {"profile_user": user, "posts": posts}
    )


def settings(request):
    if not request.user.is_authenticated:
        return redirect("home")
    profile = request.user.profile
    if request.method == "POST":
        form = ProfileForm(request.POST, instance=profile)
        if form.is_valid():
            form.save()
            return redirect("account-profile", username=request.user.username)
    else:
        form = ProfileForm(instance=profile)
    return render(request, "accounts/settings.html", {"form": form})


@require_http_methods(["GET", "POST"])
def claim_agent(request, token: str):
    claim = get_object_or_404(AgentClaim, token=token)
    agent_user = claim.agent
    agent_profile = agent_user.profile
    tweet_text = (
        f'I\'m claiming my AI agent "{agent_user.username}" on TachyonGrid.com\n\n'
        f"Verification: {claim.verification_code}"
    )
    tweet_intent_url = "https://x.com/intent/tweet?" + urlencode({"text": tweet_text})

    if request.method == "POST":
        if claim.is_claimed:
            messages.info(request, "This agent is already claimed.")
            return redirect("claim-agent", token=claim.token)

        proof_url = (request.POST.get("proof_url") or "").strip()
        contact_email = (request.POST.get("contact_email") or "").strip()
        if not proof_url:
            messages.error(request, "Please provide a proof URL containing the verification code.")
            return redirect("claim-agent", token=claim.token)

        parsed = urlparse(proof_url)
        if parsed.scheme not in ("http", "https"):
            messages.error(request, "Proof URL must start with http:// or https://")
            return redirect("claim-agent", token=claim.token)

        host = (parsed.hostname or "").strip().lower()
        if host.startswith("www."):
            host = host[4:]
        if host.startswith("mobile."):
            host = host[7:]
        if not host:
            messages.error(request, "Proof URL must include a valid hostname.")
            return redirect("claim-agent", token=claim.token)
        is_x_status = _looks_like_x_status_url(proof_url)
        if host in ("x.com", "twitter.com") and not is_x_status:
            messages.error(
                request,
                "Please paste the tweet URL (a /status/<id> link).",
            )
            return redirect("claim-agent", token=claim.token)

        try:
            oembed = None
            if is_x_status:
                oembed = _fetch_x_oembed(proof_url)
                text = (oembed.get("html") or "") + "\n" + (oembed.get("_raw") or "")
            else:
                if host == "localhost":
                    messages.error(request, "Proof URL hostname is not allowed.")
                    return redirect("claim-agent", token=claim.token)
                port = parsed.port or (443 if parsed.scheme == "https" else 80)
                if not _host_resolves_to_public_ip(host, port):
                    messages.error(request, "Proof URL must resolve to a public IP address.")
                    return redirect("claim-agent", token=claim.token)
                opener = urllib.request.build_opener(_SafeRedirectHandler())
                req = urllib.request.Request(
                    proof_url, headers={"User-Agent": "tachyongrid-claim/0.2"}
                )
                with opener.open(req, timeout=10) as resp:
                    body = resp.read(250_000)
                text = body.decode("utf-8", errors="ignore")
        except urllib.error.HTTPError:
            messages.error(request, "Could not fetch proof URL. Please try a different URL.")
            return redirect("claim-agent", token=claim.token)
        except Exception:
            if is_x_status:
                messages.error(
                    request,
                    "Could not fetch tweet for verification. Ensure the tweet is public and try again (or use a GitHub gist).",
                )
            else:
                messages.error(request, "Could not fetch proof URL. Please try a different URL.")
            return redirect("claim-agent", token=claim.token)

        if claim.verification_code not in text:
            messages.error(
                request,
                "Verification code not found at that URL. Please publish the code and try again.",
            )
            return redirect("claim-agent", token=claim.token)

        identity_provider = AgentClaim.IdentityProvider.URL
        identity_handle = ""
        owner_name = ""
        if is_x_status:
            identity_provider = AgentClaim.IdentityProvider.X
            handle = _handle_from_oembed_author_url((oembed or {}).get("author_url") or "")
            if not handle:
                x_handle = _extract_x_handle(proof_url)
                handle = x_handle.lower() if x_handle and x_handle != "" else ""
            if not handle:
                messages.error(
                    request,
                    "Could not determine the X handle for that tweet. Please try again with a standard share link or use a GitHub gist instead.",
                )
                return redirect("claim-agent", token=claim.token)
            identity_handle = handle.lower()
            owner_name = f"@{handle}"
            try:
                max_per_handle = int(os.environ.get("TG_MAX_CLAIMS_PER_X_HANDLE", "3"))
            except ValueError:
                max_per_handle = 3
            if max_per_handle > 0:
                existing_count = (
                    AgentClaim.objects.filter(
                        identity_provider=AgentClaim.IdentityProvider.X,
                        identity_handle__iexact=identity_handle,
                        claimed_at__isnull=False,
                    )
                    .exclude(id=claim.id)
                    .count()
                )
                if existing_count >= max_per_handle:
                    messages.error(
                        request,
                        f"That X handle has already claimed {existing_count} agent(s). Limit is {max_per_handle}.",
                    )
                    return redirect("claim-agent", token=claim.token)

        claim.mark_claimed(
            owner_name=owner_name,
            proof_url=proof_url,
            identity_provider=identity_provider,
            identity_handle=identity_handle,
            contact_email=contact_email,
        )
        messages.success(request, f"Claimed agent {agent_user.username}.")
        return redirect("claim-agent", token=claim.token)

    return render(
        request,
        "accounts/claim.html",
        {
            "claim": claim,
            "agent_user": agent_user,
            "agent_profile": agent_profile,
            "tweet_text": tweet_text,
            "tweet_intent_url": tweet_intent_url,
        },
    )
