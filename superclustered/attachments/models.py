import os
import uuid

from django.conf import settings
from django.db import models
from django.utils import timezone
from django.utils.text import slugify


def _attachment_upload_to(instance, filename: str) -> str:
    now = timezone.now()
    base, ext = os.path.splitext(filename)
    safe_base = slugify(base)[:64] or "file"
    safe_ext = ext[:16]
    return f"attachments/{now:%Y/%m/%d}/{instance.id}/{safe_base}{safe_ext}"


class Attachment(models.Model):
    id = models.UUIDField(primary_key=True, default=uuid.uuid4, editable=False)
    uploaded_by = models.ForeignKey(
        settings.AUTH_USER_MODEL,
        on_delete=models.PROTECT,
        related_name="attachments",
    )
    post = models.ForeignKey(
        "posts.Post",
        on_delete=models.CASCADE,
        null=True,
        blank=True,
        related_name="attachments",
    )
    comment = models.ForeignKey(
        "posts.Comment",
        on_delete=models.CASCADE,
        null=True,
        blank=True,
        related_name="attachments",
    )
    file = models.FileField(upload_to=_attachment_upload_to)
    original_name = models.CharField(max_length=255)
    content_type = models.CharField(max_length=127, blank=True)
    size_bytes = models.BigIntegerField(default=0)
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        indexes = [
            models.Index(fields=["-created_at"]),
        ]
        constraints = [
            models.CheckConstraint(
                name="attachment_exactly_one_parent",
                check=(
                    (models.Q(post__isnull=False) & models.Q(comment__isnull=True))
                    | (models.Q(post__isnull=True) & models.Q(comment__isnull=False))
                ),
            )
        ]

    def __str__(self) -> str:  # pragma: no cover
        return self.original_name
