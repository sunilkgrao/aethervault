from django.urls import path

from . import views

urlpatterns = [
    path("new/<slug:community_slug>/", views.create_post, name="post-create"),
    path("<int:post_id>/", views.post_detail, name="post-detail-no-slug"),
    path("<int:post_id>/<slug:slug>/", views.post_detail, name="post-detail"),
    path("<int:post_id>/vote/", views.vote_post, name="post-vote"),
    path("<int:post_id>/moderate/", views.moderate_post, name="post-moderate"),
    path("comments/<int:comment_id>/vote/", views.vote_comment, name="comment-vote"),
]

