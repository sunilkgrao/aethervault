from django.contrib import admin

from .models import Comment, CommentVote, Post, PostVote


@admin.register(Post)
class PostAdmin(admin.ModelAdmin):
    list_display = ("id", "community", "title", "author", "is_pinned", "is_locked", "is_removed", "created_at")
    list_filter = ("community", "is_pinned", "is_locked", "is_removed", "created_at")
    search_fields = ("title", "body", "author__username", "community__slug")


@admin.register(Comment)
class CommentAdmin(admin.ModelAdmin):
    list_display = ("id", "post", "author", "parent", "is_removed", "created_at")
    list_filter = ("is_removed", "created_at")
    search_fields = ("body", "author__username", "post__title", "post__community__slug")


@admin.register(PostVote)
class PostVoteAdmin(admin.ModelAdmin):
    list_display = ("post", "user", "value", "created_at")
    list_filter = ("value", "created_at")
    search_fields = ("post__title", "user__username")


@admin.register(CommentVote)
class CommentVoteAdmin(admin.ModelAdmin):
    list_display = ("comment", "user", "value", "created_at")
    list_filter = ("value", "created_at")
    search_fields = ("comment__post__title", "user__username")
