from collections import defaultdict
from typing import Dict, List, Optional

from django.contrib.auth.decorators import login_required
from django.db.models import Sum
from django.db.models.functions import Coalesce
from django.http import Http404, HttpResponseBadRequest, HttpResponseForbidden
from django.shortcuts import get_object_or_404, redirect, render
from django.views.decorators.http import require_POST

from attachments.models import Attachment
from communities.models import Community
from posts.models import Comment, CommentVote, Post, PostVote

from .forms import CommentForm, PostForm


# Allowed file extensions and their expected content types
ALLOWED_EXTENSIONS = {
    ".jpg": ["image/jpeg"],
    ".jpeg": ["image/jpeg"],
    ".png": ["image/png"],
    ".gif": ["image/gif"],
    ".webp": ["image/webp"],
    ".pdf": ["application/pdf"],
    ".txt": ["text/plain"],
    ".md": ["text/plain", "text/markdown"],
    ".json": ["application/json", "text/plain"],
    ".csv": ["text/csv", "text/plain"],
}

# Magic bytes for file type verification
MAGIC_BYTES = {
    b"\xff\xd8\xff": "image/jpeg",
    b"\x89PNG\r\n\x1a\n": "image/png",
    b"GIF87a": "image/gif",
    b"GIF89a": "image/gif",
    b"RIFF": "image/webp",  # WebP starts with RIFF
    b"%PDF": "application/pdf",
}


def _validate_file(f) -> bool:
    """Validate file type by extension and magic bytes."""
    name = getattr(f, "name", "").lower()
    ext = "." + name.rsplit(".", 1)[-1] if "." in name else ""

    if ext not in ALLOWED_EXTENSIONS:
        return False

    # Check magic bytes for binary files
    if ext in (".jpg", ".jpeg", ".png", ".gif", ".webp", ".pdf"):
        try:
            f.seek(0)
            header = f.read(16)
            f.seek(0)

            matched = False
            for magic, expected_type in MAGIC_BYTES.items():
                if header.startswith(magic):
                    if expected_type in ALLOWED_EXTENSIONS.get(ext, []):
                        matched = True
                        break
            if not matched:
                return False
        except Exception:
            return False

    return True


def _ensure_can_view(user, community: Community):
    if community.is_private and not community.is_member(user):
        raise Http404


def _save_attachments(files, *, uploaded_by, post=None, comment=None):
    saved = []
    for f in files:
        if not _validate_file(f):
            continue  # Skip invalid files silently
        saved.append(
            Attachment.objects.create(
                uploaded_by=uploaded_by,
                post=post,
                comment=comment,
                file=f,
                original_name=getattr(f, "name", "upload"),
                content_type=getattr(f, "content_type", "") or "",
                size_bytes=getattr(f, "size", 0) or 0,
            )
        )
    return saved


@login_required
def create_post(request, community_slug: str):
    community = get_object_or_404(Community, slug=community_slug)
    _ensure_can_view(request.user, community)

    if request.method == "POST":
        form = PostForm(request.POST, request.FILES, community=community)
        if form.is_valid():
            post = form.save(commit=False)
            post.community = community
            post.author = request.user
            post.save()
            _save_attachments(
                request.FILES.getlist("attachments"), uploaded_by=request.user, post=post
            )
            return redirect(post.get_absolute_url())
    else:
        form = PostForm(community=community)
    return render(request, "posts/create.html", {"community": community, "form": form})


def post_detail(request, post_id: int, slug: Optional[str] = None):
    post = (
        Post.objects.filter(pk=post_id, is_removed=False)
        .select_related("community", "topic", "author")
        .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
        .prefetch_related("attachments")
        .first()
    )
    if not post:
        raise Http404
    _ensure_can_view(request.user, post.community)
    if slug and slug != post.slug:
        return redirect(post.get_absolute_url())

    comments_qs = (
        Comment.objects.filter(post=post, is_removed=False)
        .select_related("author")
        .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
        .prefetch_related("attachments")
        .order_by("created_at")
    )
    comments_by_parent: Dict[Optional[int], List[Comment]] = defaultdict(list)
    for c in comments_qs:
        comments_by_parent[c.parent_id].append(c)

    reply_to = None
    if request.user.is_authenticated:
        raw_reply_to = (request.GET.get("reply_to") or "").strip()
        if raw_reply_to.isdigit():
            candidate = int(raw_reply_to)
            if Comment.objects.filter(post=post, id=candidate, is_removed=False).exists():
                reply_to = candidate

    comment_form = CommentForm(initial={"parent_id": reply_to} if reply_to else None)
    if request.method == "POST" and request.user.is_authenticated:
        if post.is_locked:
            return HttpResponseForbidden("This post is locked.")
        comment_form = CommentForm(request.POST, request.FILES)
        if comment_form.is_valid():
            parent_id = comment_form.cleaned_data.get("parent_id")
            parent = None
            if parent_id:
                parent = Comment.objects.filter(id=parent_id, post=post).first()
                if not parent:
                    return HttpResponseBadRequest("Invalid parent comment.")
            comment = Comment.objects.create(
                post=post, author=request.user, parent=parent, body=comment_form.cleaned_data["body"]
            )
            _save_attachments(
                request.FILES.getlist("attachments"),
                uploaded_by=request.user,
                comment=comment,
            )
            return redirect(post.get_absolute_url())

    membership = None
    if request.user.is_authenticated:
        membership = post.community.memberships.filter(user=request.user).first()
    return render(
        request,
        "posts/detail.html",
        {
            "post": post,
            "comment_form": comment_form,
            "comments_by_parent": comments_by_parent,
            "root_comments": comments_by_parent.get(None, []),
            "reply_to": reply_to,
            "membership": membership,
        },
    )


@login_required
@require_POST
def vote_post(request, post_id: int):
    post = get_object_or_404(Post, pk=post_id, is_removed=False)
    _ensure_can_view(request.user, post.community)
    try:
        value = int(request.POST.get("value", "0"))
    except ValueError:
        return HttpResponseBadRequest("Invalid vote value.")
    if value not in (PostVote.Value.UP, PostVote.Value.DOWN):
        return HttpResponseBadRequest("Invalid vote value.")

    vote, created = PostVote.objects.get_or_create(
        post=post, user=request.user, defaults={"value": value}
    )
    if not created:
        if vote.value == value:
            vote.delete()
        else:
            vote.value = value
            vote.save(update_fields=["value"])
    return redirect(post.get_absolute_url())


@login_required
@require_POST
def vote_comment(request, comment_id: int):
    comment = get_object_or_404(Comment, pk=comment_id, is_removed=False)
    post = comment.post
    _ensure_can_view(request.user, post.community)
    try:
        value = int(request.POST.get("value", "0"))
    except ValueError:
        return HttpResponseBadRequest("Invalid vote value.")
    if value not in (CommentVote.Value.UP, CommentVote.Value.DOWN):
        return HttpResponseBadRequest("Invalid vote value.")

    vote, created = CommentVote.objects.get_or_create(
        comment=comment, user=request.user, defaults={"value": value}
    )
    if not created:
        if vote.value == value:
            vote.delete()
        else:
            vote.value = value
            vote.save(update_fields=["value"])
    return redirect(post.get_absolute_url())


@login_required
@require_POST
def moderate_post(request, post_id: int):
    post = get_object_or_404(Post, pk=post_id)
    community = post.community
    if not community.is_moderator(request.user):
        return HttpResponseForbidden("Moderator access required.")
    action = (request.POST.get("action") or "").strip()
    if action == "pin":
        post.is_pinned = True
    elif action == "unpin":
        post.is_pinned = False
    elif action == "lock":
        post.is_locked = True
    elif action == "unlock":
        post.is_locked = False
    elif action == "remove":
        post.is_removed = True
    elif action == "restore":
        post.is_removed = False
    else:
        return HttpResponseBadRequest("Unknown action.")
    post.save(update_fields=["is_pinned", "is_locked", "is_removed", "updated_at"])
    return redirect(post.get_absolute_url())
