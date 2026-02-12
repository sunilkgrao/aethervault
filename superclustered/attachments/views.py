import mimetypes

from django.http import FileResponse, Http404
from django.shortcuts import get_object_or_404
from django.utils.text import get_valid_filename

from .models import Attachment


def download_attachment(request, attachment_id):
    attachment = get_object_or_404(Attachment, id=attachment_id)

    community = None
    is_removed = False
    if attachment.post_id:
        community = attachment.post.community
        is_removed = attachment.post.is_removed
    elif attachment.comment_id:
        community = attachment.comment.post.community
        is_removed = attachment.comment.is_removed or attachment.comment.post.is_removed

    if not community:
        raise Http404
    if community.is_private and not community.is_member(request.user):
        raise Http404
    if is_removed and not community.is_moderator(request.user):
        raise Http404

    content_type = attachment.content_type or mimetypes.guess_type(attachment.original_name)[
        0
    ] or "application/octet-stream"

    try:
        fh = attachment.file.open("rb")
    except FileNotFoundError:
        raise Http404

    response = FileResponse(fh, content_type=content_type)
    response["Content-Length"] = str(attachment.size_bytes or 0)
    safe_name = get_valid_filename(attachment.original_name)[:200] or "download"
    response["Content-Disposition"] = f'attachment; filename="{safe_name}"'
    # Security headers to prevent content sniffing and XSS
    response["X-Content-Type-Options"] = "nosniff"
    response["Cache-Control"] = "private, no-cache"
    return response
