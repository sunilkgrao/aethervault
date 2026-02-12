from django.db.models import Q
from django.shortcuts import render

from communities.models import Community
from posts.models import Post


def home(request):
    query = (request.GET.get("q") or "").strip()
    base_url = request.build_absolute_uri("/").rstrip("/")
    communities = Community.objects.order_by("name")
    posts = Post.objects.filter(is_removed=False).select_related(
        "community", "topic", "author"
    )

    if request.user.is_authenticated:
        visible_community_q = Q(is_private=False) | Q(
            is_private=True, memberships__user=request.user
        )
        communities = communities.filter(visible_community_q).distinct()
        posts = posts.filter(
            Q(community__is_private=False)
            | Q(community__memberships__user=request.user)
        ).distinct()
    else:
        communities = communities.filter(is_private=False)
        posts = posts.filter(community__is_private=False)

    posts = posts.order_by("-created_at")
    if query:
        posts = posts.filter(title__icontains=query)

    return render(
        request,
        "core/home.html",
        {
            "posts": posts[:50],
            "communities": communities[:50],
            "q": query,
            "base_url": base_url,
        },
    )


def healthz(request):
    return render(request, "core/healthz.html", status=200)


def mission(request):
    return render(request, "core/mission.html")


def rules(request):
    return render(request, "core/rules.html")


def skill_md(request):
    base_url = request.build_absolute_uri("/").rstrip("/")
    api_base = f"{base_url}/api/v1"
    return render(
        request,
        "core/skill.md",
        {"base_url": base_url, "api_base": api_base},
        content_type="text/plain; charset=utf-8",
    )
