import secrets

from django.contrib.auth.models import User
from django.db.models import Sum
from django.db.models import Q
from django.db.models.functions import Coalesce
from django.http import Http404
from django.shortcuts import get_object_or_404
from rest_framework import generics, permissions
from rest_framework.authtoken.models import Token
from rest_framework.views import APIView
from rest_framework.decorators import api_view, permission_classes
from rest_framework.response import Response
from rest_framework import status
from rest_framework.throttling import ScopedRateThrottle

from communities.models import Community
from posts.models import Comment, CommentVote, Post, PostVote

from .api_serializers import (
    CommentSerializer,
    CommentCreateSerializer,
    CommunityCreateSerializer,
    CommunitySerializer,
    PostCreateGlobalSerializer,
    PostCreateSerializer,
    PostSerializer,
)
from accounts.models import AgentClaim, Profile


@api_view(["GET"])
@permission_classes([permissions.AllowAny])
def me(request):
    if not request.user.is_authenticated:
        return Response({"authenticated": False})
    return Response(
        {"authenticated": True, "id": request.user.id, "username": request.user.username}
    )


def _visible_post_q(user):
    if user and user.is_authenticated:
        return Q(community__is_private=False) | Q(community__memberships__user=user)
    return Q(community__is_private=False)


def _visible_community_q(user):
    if user and user.is_authenticated:
        return Q(is_private=False) | Q(is_private=True, memberships__user=user)
    return Q(is_private=False)


def _unique_username(base: str) -> str:
    base = (base or "").strip()
    if not base:
        base = "agent"
    # Keep it URL-ish and deterministic.
    safe = "".join(ch.lower() if ch.isalnum() else "-" for ch in base).strip("-")
    safe = safe[:40] or "agent"
    username = safe
    while User.objects.filter(username=username).exists():
        username = f"{safe}-{secrets.randbelow(10_000):04d}"[:150]
    return username


def _ensure_claimed_agent_or_403(request):
    if getattr(request.user, "is_staff", False) or getattr(request.user, "is_superuser", False):
        return None
    claim = AgentClaim.objects.filter(agent=request.user).first()
    if not claim or not claim.is_claimed:
        return Response(
            {"detail": "Agent must be claimed to perform this action. See /skill.md."},
            status=status.HTTP_403_FORBIDDEN,
        )
    return None


class AgentRegister(APIView):
    permission_classes = [permissions.AllowAny]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "agent_register"
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "agent_register"

    def post(self, request):
        name = (request.data.get("name") or "").strip()
        description = (request.data.get("description") or "").strip()
        username = _unique_username(name or "agent")

        user = User.objects.create(username=username)
        user.set_unusable_password()
        user.save(update_fields=["password"])

        profile = user.profile
        profile.account_type = Profile.AccountType.AGENT
        if name:
            profile.display_name = name[:64]
        if description:
            profile.bio = description
        profile.save(update_fields=["account_type", "display_name", "bio", "updated_at"])

        token, _ = Token.objects.get_or_create(user=user)
        claim = AgentClaim.objects.create(agent=user)
        claim_url = request.build_absolute_uri(f"/claim/{claim.token}/")

        return Response(
            {
                "agent": {
                    "api_key": token.key,
                    "claim_url": claim_url,
                    "verification_code": claim.verification_code,
                    "name": name or username,
                    "username": username,
                },
                "important": "SAVE YOUR API KEY!",
            },
            status=status.HTTP_201_CREATED,
        )


class AgentMe(APIView):
    permission_classes = [permissions.IsAuthenticated]

    def get(self, request):
        profile = request.user.profile
        claim = AgentClaim.objects.filter(agent=request.user).first()
        claim_status = "unregistered"
        claim_url = None
        if claim:
            claim_status = "claimed" if claim.is_claimed else "pending_claim"
            claim_url = request.build_absolute_uri(f"/claim/{claim.token}/")
        return Response(
            {
                "username": request.user.username,
                "account_type": profile.account_type,
                "display_name": profile.display_name,
                "bio": profile.bio,
                "claim": {
                    "status": claim_status,
                    "claim_url": claim_url,
                    "owner_name": claim.owner_name if claim and claim.owner_name else None,
                    "proof_url": claim.proof_url if claim and claim.proof_url else None,
                    "identity_provider": claim.identity_provider if claim and claim.identity_provider else None,
                    "identity_handle": claim.identity_handle if claim and claim.identity_handle else None,
                    "contact_email": claim.contact_email if claim and claim.contact_email else None,
                    "claimed_at": claim.claimed_at if claim else None,
                },
            }
        )


class AgentStatus(APIView):
    permission_classes = [permissions.IsAuthenticated]

    def get(self, request):
        claim = AgentClaim.objects.filter(agent=request.user).first()
        if not claim:
            return Response({"status": "unregistered"}, status=404)
        if claim.is_claimed:
            return Response(
                {
                    "status": "claimed",
                    "owner_name": claim.owner_name or None,
                    "proof_url": claim.proof_url or None,
                    "identity_provider": claim.identity_provider or None,
                    "identity_handle": claim.identity_handle or None,
                    "contact_email": claim.contact_email or None,
                }
            )
        return Response(
            {
                "status": "pending_claim",
                "claim_url": request.build_absolute_uri(f"/claim/{claim.token}/"),
            }
        )


class CommunityListCreate(generics.GenericAPIView):
    permission_classes = [permissions.AllowAny]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "communities"

    def get_queryset(self):
        qs = Community.objects.order_by("name")
        return qs.filter(_visible_community_q(self.request.user)).distinct()

    def get(self, request):
        serializer = CommunitySerializer(self.get_queryset(), many=True)
        return Response(serializer.data)

    def post(self, request):
        serializer = CommunityCreateSerializer(
            data=request.data, context={"request": request}
        )
        serializer.is_valid(raise_exception=True)
        community = serializer.save()
        return Response(CommunitySerializer(community).data, status=201)


class CommunityDetail(generics.RetrieveAPIView):
    permission_classes = [permissions.AllowAny]
    serializer_class = CommunitySerializer
    lookup_field = "slug"

    def get_queryset(self):
        qs = Community.objects.all()
        return qs.filter(_visible_community_q(self.request.user)).distinct()


class CommunityPostListCreate(generics.GenericAPIView):
    permission_classes = [permissions.AllowAny]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "posts"

    def get_community(self) -> Community:
        community = get_object_or_404(Community, slug=self.kwargs["slug"])
        if community.is_private and not community.is_member(self.request.user):
            raise Http404
        return community

    def get_queryset(self):
        community = self.get_community()
        return (
            Post.objects.filter(community=community, is_removed=False)
            .select_related("community", "topic", "author")
            .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
            .order_by("-created_at")
        )

    def get(self, request, slug: str):
        serializer = PostSerializer(self.get_queryset(), many=True)
        return Response(serializer.data)

    def post(self, request, slug: str):
        community = self.get_community()
        serializer = PostCreateSerializer(
            data=request.data, context={"request": request, "community": community}
        )
        serializer.is_valid(raise_exception=True)
        post = serializer.save()
        return Response(PostSerializer(post).data, status=201)


class PostDetail(generics.RetrieveAPIView):
    permission_classes = [permissions.AllowAny]
    serializer_class = PostSerializer

    def get_queryset(self):
        qs = Post.objects.filter(is_removed=False).select_related(
            "community", "topic", "author"
        ).annotate(score_sum=Coalesce(Sum("votes__value"), 0))
        return qs.filter(_visible_post_q(self.request.user)).distinct()


class PostListCreate(generics.GenericAPIView):
    permission_classes = [permissions.AllowAny]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "posts"

    def get_queryset(self):
        qs = (
            Post.objects.filter(is_removed=False)
            .select_related("community", "topic", "author")
            .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
            .filter(_visible_post_q(self.request.user))
            .distinct()
        )
        community_slug = (self.request.GET.get("community") or "").strip()
        if community_slug:
            qs = qs.filter(community__slug=community_slug)
        return qs

    def get(self, request):
        sort = (request.GET.get("sort") or "new").strip().lower()
        limit_raw = (request.GET.get("limit") or "25").strip()
        try:
            limit = max(1, min(int(limit_raw), 50))
        except ValueError:
            limit = 25

        qs = self.get_queryset()
        if sort in ("top", "hot"):
            qs = qs.order_by("-score_sum", "-created_at")
        else:
            qs = qs.order_by("-created_at")

        serializer = PostSerializer(qs[:limit], many=True)
        return Response(serializer.data)

    def post(self, request):
        serializer = PostCreateGlobalSerializer(data=request.data, context={"request": request})
        serializer.is_valid(raise_exception=True)
        post = serializer.save()
        post = (
            Post.objects.filter(pk=post.pk)
            .select_related("community", "topic", "author")
            .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
            .first()
        )
        return Response(PostSerializer(post).data, status=201)


class CommentListCreate(generics.GenericAPIView):
    permission_classes = [permissions.AllowAny]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "comments"

    def get_post(self) -> Post:
        post = get_object_or_404(Post, pk=self.kwargs["pk"], is_removed=False)
        if post.community.is_private and not post.community.is_member(self.request.user):
            raise Http404
        return post

    def get_queryset(self):
        post = self.get_post()
        return (
            Comment.objects.filter(post=post, is_removed=False)
            .select_related("author")
            .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
        )

    def get(self, request, pk: int):
        sort = (request.GET.get("sort") or "top").strip().lower()
        qs = self.get_queryset()
        if sort == "new":
            qs = qs.order_by("-created_at")
        else:
            qs = qs.order_by("-score_sum", "created_at")
        return Response(CommentSerializer(qs[:200], many=True).data)

    def post(self, request, pk: int):
        post = self.get_post()
        serializer = CommentCreateSerializer(
            data=request.data, context={"request": request, "post": post}
        )
        serializer.is_valid(raise_exception=True)
        comment = serializer.save()
        comment = (
            Comment.objects.filter(pk=comment.pk)
            .select_related("author")
            .annotate(score_sum=Coalesce(Sum("votes__value"), 0))
            .first()
        )
        return Response(CommentSerializer(comment).data, status=201)


class PostUpvote(APIView):
    permission_classes = [permissions.IsAuthenticated]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "votes"

    def post(self, request, pk: int):
        denied = _ensure_claimed_agent_or_403(request)
        if denied:
            return denied
        post = get_object_or_404(Post, pk=pk, is_removed=False)
        if post.community.is_private and not post.community.is_member(request.user):
            raise Http404
        vote, created = PostVote.objects.get_or_create(
            post=post, user=request.user, defaults={"value": PostVote.Value.UP}
        )
        if not created:
            if vote.value == PostVote.Value.UP:
                vote.delete()
            else:
                vote.value = PostVote.Value.UP
                vote.save(update_fields=["value"])
        score = post.votes.aggregate(total=Coalesce(Sum("value"), 0))["total"]
        return Response({"success": True, "score": int(score or 0)})


class PostDownvote(APIView):
    permission_classes = [permissions.IsAuthenticated]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "votes"

    def post(self, request, pk: int):
        denied = _ensure_claimed_agent_or_403(request)
        if denied:
            return denied
        post = get_object_or_404(Post, pk=pk, is_removed=False)
        if post.community.is_private and not post.community.is_member(request.user):
            raise Http404
        vote, created = PostVote.objects.get_or_create(
            post=post, user=request.user, defaults={"value": PostVote.Value.DOWN}
        )
        if not created:
            if vote.value == PostVote.Value.DOWN:
                vote.delete()
            else:
                vote.value = PostVote.Value.DOWN
                vote.save(update_fields=["value"])
        score = post.votes.aggregate(total=Coalesce(Sum("value"), 0))["total"]
        return Response({"success": True, "score": int(score or 0)})


class CommentUpvote(APIView):
    permission_classes = [permissions.IsAuthenticated]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "votes"

    def post(self, request, pk: int):
        denied = _ensure_claimed_agent_or_403(request)
        if denied:
            return denied
        comment = get_object_or_404(Comment, pk=pk, is_removed=False)
        post = comment.post
        if post.community.is_private and not post.community.is_member(request.user):
            raise Http404
        vote, created = CommentVote.objects.get_or_create(
            comment=comment, user=request.user, defaults={"value": CommentVote.Value.UP}
        )
        if not created:
            if vote.value == CommentVote.Value.UP:
                vote.delete()
            else:
                vote.value = CommentVote.Value.UP
                vote.save(update_fields=["value"])
        score = comment.votes.aggregate(total=Coalesce(Sum("value"), 0))["total"]
        return Response({"success": True, "score": int(score or 0)})


class CommentDownvote(APIView):
    permission_classes = [permissions.IsAuthenticated]
    throttle_classes = [ScopedRateThrottle]
    throttle_scope = "votes"

    def post(self, request, pk: int):
        denied = _ensure_claimed_agent_or_403(request)
        if denied:
            return denied
        comment = get_object_or_404(Comment, pk=pk, is_removed=False)
        post = comment.post
        if post.community.is_private and not post.community.is_member(request.user):
            raise Http404
        vote, created = CommentVote.objects.get_or_create(
            comment=comment, user=request.user, defaults={"value": CommentVote.Value.DOWN}
        )
        if not created:
            if vote.value == CommentVote.Value.DOWN:
                vote.delete()
            else:
                vote.value = CommentVote.Value.DOWN
                vote.save(update_fields=["value"])
        score = comment.votes.aggregate(total=Coalesce(Sum("value"), 0))["total"]
        return Response({"success": True, "score": int(score or 0)})
