from django.conf import settings
from django.db import models
from django.db.models import Sum
from django.utils.text import slugify

from core.markdown import render_markdown


class Post(models.Model):
    community = models.ForeignKey(
        "communities.Community", on_delete=models.CASCADE, related_name="posts"
    )
    topic = models.ForeignKey(
        "communities.Topic",
        on_delete=models.SET_NULL,
        null=True,
        blank=True,
        related_name="posts",
    )
    author = models.ForeignKey(
        settings.AUTH_USER_MODEL, on_delete=models.PROTECT, related_name="posts"
    )
    title = models.CharField(max_length=200)
    slug = models.SlugField(max_length=80)
    body = models.TextField(blank=True)
    is_pinned = models.BooleanField(default=False)
    is_locked = models.BooleanField(default=False)
    is_removed = models.BooleanField(default=False)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    class Meta:
        indexes = [
            models.Index(fields=["community", "-created_at"]),
            models.Index(fields=["-created_at"]),
        ]

    def save(self, *args, **kwargs):  # pragma: no cover
        if not self.slug:
            self.slug = slugify(self.title)[:80] or "post"
        super().save(*args, **kwargs)

    def __str__(self) -> str:  # pragma: no cover
        return f"{self.title} (c/{self.community.slug})"

    def get_absolute_url(self) -> str:
        return f"/posts/{self.id}/{self.slug}/"

    @property
    def body_html(self) -> str:
        return render_markdown(self.body)

    @property
    def score(self) -> int:
        total = self.votes.aggregate(total=Sum("value"))["total"]
        return int(total or 0)


class Comment(models.Model):
    post = models.ForeignKey(Post, on_delete=models.CASCADE, related_name="comments")
    author = models.ForeignKey(
        settings.AUTH_USER_MODEL, on_delete=models.PROTECT, related_name="comments"
    )
    parent = models.ForeignKey(
        "self", on_delete=models.CASCADE, null=True, blank=True, related_name="replies"
    )
    body = models.TextField()
    is_removed = models.BooleanField(default=False)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    class Meta:
        indexes = [
            models.Index(fields=["post", "-created_at"]),
        ]

    def __str__(self) -> str:  # pragma: no cover
        return f"Comment by {self.author.get_username()} on {self.post_id}"

    @property
    def body_html(self) -> str:
        return render_markdown(self.body)

    @property
    def score(self) -> int:
        total = self.votes.aggregate(total=Sum("value"))["total"]
        return int(total or 0)


class PostVote(models.Model):
    class Value(models.IntegerChoices):
        UP = 1, "Upvote"
        DOWN = -1, "Downvote"

    post = models.ForeignKey(Post, on_delete=models.CASCADE, related_name="votes")
    user = models.ForeignKey(
        settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name="post_votes"
    )
    value = models.SmallIntegerField(choices=Value.choices)
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        constraints = [
            models.UniqueConstraint(fields=["post", "user"], name="uniq_postvote_user")
        ]


class CommentVote(models.Model):
    class Value(models.IntegerChoices):
        UP = 1, "Upvote"
        DOWN = -1, "Downvote"

    comment = models.ForeignKey(Comment, on_delete=models.CASCADE, related_name="votes")
    user = models.ForeignKey(
        settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name="comment_votes"
    )
    value = models.SmallIntegerField(choices=Value.choices)
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        constraints = [
            models.UniqueConstraint(
                fields=["comment", "user"], name="uniq_commentvote_user"
            )
        ]
