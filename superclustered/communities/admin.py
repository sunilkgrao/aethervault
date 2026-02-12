from django.contrib import admin

from .models import Community, CommunityMembership, Topic


@admin.register(Community)
class CommunityAdmin(admin.ModelAdmin):
    list_display = ("slug", "name", "is_private", "created_by", "created_at")
    list_filter = ("is_private", "created_at")
    search_fields = ("slug", "name", "description", "created_by__username")
    prepopulated_fields = {"slug": ("name",)}


@admin.register(Topic)
class TopicAdmin(admin.ModelAdmin):
    list_display = ("community", "slug", "name", "created_by", "created_at")
    list_filter = ("created_at",)
    search_fields = ("community__slug", "slug", "name", "description")
    prepopulated_fields = {"slug": ("name",)}


@admin.register(CommunityMembership)
class CommunityMembershipAdmin(admin.ModelAdmin):
    list_display = ("community", "user", "role", "created_at")
    list_filter = ("role", "created_at")
    search_fields = ("community__slug", "user__username")
