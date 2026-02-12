from django.contrib import admin

from .models import Attachment


@admin.register(Attachment)
class AttachmentAdmin(admin.ModelAdmin):
    list_display = ("id", "original_name", "uploaded_by", "post", "comment", "size_bytes", "created_at")
    list_filter = ("created_at",)
    search_fields = ("original_name", "uploaded_by__username")
