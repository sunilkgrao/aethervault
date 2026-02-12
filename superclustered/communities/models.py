from django.conf import settings
from django.db import models
from django.utils.text import slugify


class Community(models.Model):
    slug = models.SlugField(max_length=50, unique=True)
    name = models.CharField(max_length=80, unique=True)
    description = models.TextField(blank=True)
    is_private = models.BooleanField(default=False)
    created_by = models.ForeignKey(
        settings.AUTH_USER_MODEL,
        on_delete=models.PROTECT,
        related_name="communities_created",
    )
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    def save(self, *args, **kwargs):  # pragma: no cover
        if not self.slug:
            base = slugify(self.name)[:50] or "community"
            slug = base
            i = 2
            while Community.objects.filter(slug=slug).exclude(pk=self.pk).exists():
                suffix = f"-{i}"
                slug = (base[: 50 - len(suffix)] + suffix).strip("-")
                i += 1
            self.slug = slug
        super().save(*args, **kwargs)

    def __str__(self) -> str:  # pragma: no cover
        return f"c/{self.slug}"

    def is_member(self, user) -> bool:
        if not user or not user.is_authenticated:
            return False
        return self.memberships.filter(user=user).exists()

    def is_moderator(self, user) -> bool:
        if not user or not user.is_authenticated:
            return False
        return self.memberships.filter(
            user=user,
            role__in=[
                CommunityMembership.Role.OWNER,
                CommunityMembership.Role.MODERATOR,
            ],
        ).exists()


class CommunityMembership(models.Model):
    class Role(models.TextChoices):
        OWNER = "owner", "Owner"
        MODERATOR = "moderator", "Moderator"
        MEMBER = "member", "Member"

    user = models.ForeignKey(
        settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name="memberships"
    )
    community = models.ForeignKey(
        Community, on_delete=models.CASCADE, related_name="memberships"
    )
    role = models.CharField(max_length=16, choices=Role.choices, default=Role.MEMBER)
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        constraints = [
            models.UniqueConstraint(
                fields=["user", "community"], name="uniq_membership_user_community"
            )
        ]

    def __str__(self) -> str:  # pragma: no cover
        return f"{self.user.get_username()} in c/{self.community.slug} ({self.role})"


class Topic(models.Model):
    community = models.ForeignKey(
        Community, on_delete=models.CASCADE, related_name="topics"
    )
    slug = models.SlugField(max_length=50)
    name = models.CharField(max_length=80)
    description = models.TextField(blank=True)
    created_by = models.ForeignKey(
        settings.AUTH_USER_MODEL,
        on_delete=models.PROTECT,
        related_name="topics_created",
    )
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        constraints = [
            models.UniqueConstraint(
                fields=["community", "slug"], name="uniq_topic_community_slug"
            ),
            models.UniqueConstraint(
                fields=["community", "name"], name="uniq_topic_community_name"
            ),
        ]

    def save(self, *args, **kwargs):  # pragma: no cover
        if not self.slug:
            base = slugify(self.name)[:50] or "topic"
            slug = base
            i = 2
            while Topic.objects.filter(community=self.community, slug=slug).exclude(
                pk=self.pk
            ).exists():
                suffix = f"-{i}"
                slug = (base[: 50 - len(suffix)] + suffix).strip("-")
                i += 1
            self.slug = slug
        super().save(*args, **kwargs)

    def __str__(self) -> str:  # pragma: no cover
        return f"c/{self.community.slug}::{self.slug}"
