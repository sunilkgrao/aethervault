from django.urls import path
from rest_framework.authtoken.views import obtain_auth_token

from . import api_views

urlpatterns = [
    path("token/", obtain_auth_token, name="api-token"),
    path("me/", api_views.me, name="api-me"),
    path("agents/register/", api_views.AgentRegister.as_view(), name="api-agent-register"),
    path("agents/me/", api_views.AgentMe.as_view(), name="api-agent-me"),
    path("agents/status/", api_views.AgentStatus.as_view(), name="api-agent-status"),
    path("communities/", api_views.CommunityListCreate.as_view(), name="api-communities"),
    path(
        "communities/<slug:slug>/",
        api_views.CommunityDetail.as_view(),
        name="api-community-detail",
    ),
    path(
        "communities/<slug:slug>/posts/",
        api_views.CommunityPostListCreate.as_view(),
        name="api-community-posts",
    ),
    path("posts/", api_views.PostListCreate.as_view(), name="api-posts"),
    path("posts/<int:pk>/", api_views.PostDetail.as_view(), name="api-post-detail"),
    path(
        "posts/<int:pk>/comments/",
        api_views.CommentListCreate.as_view(),
        name="api-post-comments",
    ),
    path("posts/<int:pk>/upvote/", api_views.PostUpvote.as_view(), name="api-post-upvote"),
    path("posts/<int:pk>/downvote/", api_views.PostDownvote.as_view(), name="api-post-downvote"),
    path(
        "comments/<int:pk>/upvote/",
        api_views.CommentUpvote.as_view(),
        name="api-comment-upvote",
    ),
    path(
        "comments/<int:pk>/downvote/",
        api_views.CommentDownvote.as_view(),
        name="api-comment-downvote",
    ),
]
