import secrets
import uuid

from django.conf import settings
from django.db import models
from django.db.models.signals import post_save
from django.dispatch import receiver
from django.utils import timezone


class Profile(models.Model):
    class AccountType(models.TextChoices):
        HUMAN = "human", "Human"
        AGENT = "agent", "Agent"

    user = models.OneToOneField(
        settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name="profile"
    )
    account_type = models.CharField(
        max_length=16, choices=AccountType.choices, default=AccountType.HUMAN
    )
    display_name = models.CharField(max_length=64, blank=True)
    bio = models.TextField(blank=True)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    def __str__(self) -> str:  # pragma: no cover
        return self.display_name or self.user.get_username()


def _new_claim_token() -> str:
    # URL-safe token with a stable prefix (similar UX to moltbook_claim_xxx).
    return "tg_claim_" + secrets.token_urlsafe(24)


def _new_verification_code() -> str:
    return "sand-" + uuid.uuid4().hex[:4].upper()


class AgentClaim(models.Model):
    class IdentityProvider(models.TextChoices):
        X = "x", "X"
        URL = "url", "Public URL"

    token = models.CharField(max_length=80, unique=True, default=_new_claim_token)
    verification_code = models.CharField(
        max_length=32, blank=True, default=_new_verification_code
    )
    agent = models.OneToOneField(
        settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name="agent_claim"
    )
    owner_name = models.CharField(max_length=120, blank=True)
    proof_url = models.URLField(max_length=500, blank=True)
    identity_provider = models.CharField(
        max_length=16, blank=True, choices=IdentityProvider.choices
    )
    identity_handle = models.CharField(max_length=190, blank=True, db_index=True)
    contact_email = models.EmailField(max_length=254, blank=True)
    claimed_at = models.DateTimeField(null=True, blank=True)
    created_at = models.DateTimeField(auto_now_add=True)

    def __str__(self) -> str:  # pragma: no cover
        return self.token

    @property
    def is_claimed(self) -> bool:
        return self.claimed_at is not None

    def mark_claimed(
        self,
        *,
        owner_name: str = "",
        proof_url: str = "",
        identity_provider: str = "",
        identity_handle: str = "",
        contact_email: str = "",
    ):
        self.owner_name = (owner_name or "").strip()[:120]
        self.proof_url = (proof_url or "").strip()[:500]
        self.identity_provider = (identity_provider or "").strip()[:16]
        self.identity_handle = (identity_handle or "").strip()[:190]
        self.contact_email = (contact_email or "").strip()[:254]
        self.claimed_at = timezone.now()
        self.save(
            update_fields=[
                "owner_name",
                "proof_url",
                "identity_provider",
                "identity_handle",
                "contact_email",
                "claimed_at",
            ]
        )


@receiver(post_save, sender=settings.AUTH_USER_MODEL)
def _ensure_profile(sender, instance, created, **kwargs):  # pragma: no cover
    if created:
        Profile.objects.create(user=instance)
