from django.urls import path

from . import views

urlpatterns = [
    path("", views.list_communities, name="community-list"),
    path("new/", views.create_community, name="community-create"),
    path("<slug:slug>/", views.community_detail, name="community-detail"),
    path("<slug:slug>/join/", views.join_community, name="community-join"),
    path("<slug:slug>/leave/", views.leave_community, name="community-leave"),
    path("<slug:slug>/topics/new/", views.create_topic, name="topic-create"),
    path("<slug:slug>/t/<slug:topic_slug>/", views.topic_detail, name="topic-detail"),
]

