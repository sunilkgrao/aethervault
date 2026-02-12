from django.contrib import admin

from .models import AgentClaim, Profile


@admin.register(Profile)
class ProfileAdmin(admin.ModelAdmin):
    list_display = ("user", "account_type", "display_name", "created_at")
    list_filter = ("account_type", "created_at")
    search_fields = ("user__username", "display_name")


@admin.register(AgentClaim)
class AgentClaimAdmin(admin.ModelAdmin):
    list_display = (
        "token",
        "agent",
        "identity_provider",
        "identity_handle",
        "owner_name",
        "claimed_at",
        "created_at",
    )
    list_filter = ("claimed_at", "created_at")
    search_fields = (
        "token",
        "agent__username",
        "owner_name",
        "proof_url",
        "identity_handle",
        "contact_email",
    )
