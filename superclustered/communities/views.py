from django.contrib.auth.decorators import login_required
from django.db.models import Q
from django.http import Http404, HttpResponseForbidden
from django.shortcuts import get_object_or_404, redirect, render
from django.views.decorators.http import require_POST

from posts.models import Post

from .forms import CommunityForm, TopicForm
from .models import Community, CommunityMembership, Topic


def _visible_communities_for(request):
    qs = Community.objects.order_by("name")
    if request.user.is_authenticated:
        return qs.filter(
            Q(is_private=False) | Q(is_private=True, memberships__user=request.user)
        ).distinct()
    return qs.filter(is_private=False)


def list_communities(request):
    return render(
        request,
        "communities/list.html",
        {"communities": _visible_communities_for(request)},
    )


@login_required
def create_community(request):
    if request.method == "POST":
        form = CommunityForm(request.POST)
        if form.is_valid():
            community = form.save(commit=False)
            community.created_by = request.user
            community.save()
            CommunityMembership.objects.create(
                user=request.user,
                community=community,
                role=CommunityMembership.Role.OWNER,
            )
            return redirect("community-detail", slug=community.slug)
    else:
        form = CommunityForm()
    return render(request, "communities/create.html", {"form": form})


def community_detail(request, slug: str):
    community = get_object_or_404(Community, slug=slug)
    if community.is_private and not community.is_member(request.user):
        raise Http404

    posts = (
        Post.objects.filter(community=community, is_removed=False)
        .select_related("author", "topic")
        .order_by("-is_pinned", "-created_at")[:50]
    )
    topics = Topic.objects.filter(community=community).order_by("name")
    membership = None
    if request.user.is_authenticated:
        membership = CommunityMembership.objects.filter(
            user=request.user, community=community
        ).first()
    return render(
        request,
        "communities/detail.html",
        {
            "community": community,
            "posts": posts,
            "topics": topics,
            "membership": membership,
        },
    )


def topic_detail(request, slug: str, topic_slug: str):
    community = get_object_or_404(Community, slug=slug)
    if community.is_private and not community.is_member(request.user):
        raise Http404
    topic = get_object_or_404(Topic, community=community, slug=topic_slug)
    posts = (
        Post.objects.filter(community=community, topic=topic, is_removed=False)
        .select_related("author", "topic")
        .order_by("-is_pinned", "-created_at")[:50]
    )
    return render(
        request,
        "communities/topic_detail.html",
        {"community": community, "topic": topic, "posts": posts},
    )


@login_required
def create_topic(request, slug: str):
    community = get_object_or_404(Community, slug=slug)
    if not community.is_moderator(request.user):
        return HttpResponseForbidden("Only community moderators can create topics.")

    if request.method == "POST":
        form = TopicForm(request.POST)
        if form.is_valid():
            topic = form.save(commit=False)
            topic.community = community
            topic.created_by = request.user
            topic.save()
            return redirect("community-detail", slug=community.slug)
    else:
        form = TopicForm()
    return render(
        request,
        "communities/topic_create.html",
        {"community": community, "form": form},
    )


@login_required
@require_POST
def join_community(request, slug: str):
    community = get_object_or_404(Community, slug=slug)
    if community.is_private:
        return HttpResponseForbidden("This community is private.")
    CommunityMembership.objects.get_or_create(
        user=request.user,
        community=community,
        defaults={"role": CommunityMembership.Role.MEMBER},
    )
    return redirect("community-detail", slug=community.slug)


@login_required
@require_POST
def leave_community(request, slug: str):
    community = get_object_or_404(Community, slug=slug)
    membership = CommunityMembership.objects.filter(
        user=request.user, community=community
    ).first()
    if not membership:
        return redirect("community-detail", slug=community.slug)
    if membership.role == CommunityMembership.Role.OWNER:
        return HttpResponseForbidden("Owners cannot leave their own community.")
    membership.delete()
    return redirect("community-detail", slug=community.slug)
