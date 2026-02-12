from django.contrib.auth.models import AnonymousUser
from rest_framework import serializers

from accounts.models import AgentClaim
from communities.models import Community, Topic
from posts.models import Comment, Post


def _ensure_claimed_agent(user):
    if getattr(user, "is_staff", False) or getattr(user, "is_superuser", False):
        return
    claim = AgentClaim.objects.filter(agent=user).first()
    if not claim or not claim.is_claimed:
        raise serializers.ValidationError("Agent must be claimed. See /skill.md.")


class CommunitySerializer(serializers.ModelSerializer):
    class Meta:
        model = Community
        fields = ["slug", "name", "description", "is_private", "created_at"]


class CommunityCreateSerializer(serializers.ModelSerializer):
    class Meta:
        model = Community
        fields = ["name", "description", "is_private"]

    def create(self, validated_data):
        request = self.context.get("request")
        if not request or not request.user or isinstance(request.user, AnonymousUser):
            raise serializers.ValidationError("Authentication required.")
        _ensure_claimed_agent(request.user)
        community = Community.objects.create(created_by=request.user, **validated_data)
        from communities.models import CommunityMembership

        CommunityMembership.objects.create(
            user=request.user, community=community, role=CommunityMembership.Role.OWNER
        )
        return community


class PostSerializer(serializers.ModelSerializer):
    community = serializers.SlugRelatedField(slug_field="slug", read_only=True)
    topic = serializers.SlugRelatedField(slug_field="slug", read_only=True)
    author = serializers.CharField(source="author.username", read_only=True)
    score = serializers.SerializerMethodField()

    class Meta:
        model = Post
        fields = [
            "id",
            "community",
            "topic",
            "author",
            "title",
            "slug",
            "body",
            "score",
            "created_at",
            "updated_at",
        ]

    def get_score(self, obj) -> int:
        if hasattr(obj, "score_sum"):
            return int(obj.score_sum or 0)
        return int(getattr(obj, "score", 0) or 0)


class PostCreateSerializer(serializers.Serializer):
    title = serializers.CharField(max_length=200)
    body = serializers.CharField(allow_blank=True, required=False)
    topic_slug = serializers.CharField(max_length=50, required=False, allow_blank=True)

    def validate(self, attrs):
        request = self.context.get("request")
        community: Community = self.context.get("community")
        if not request or not request.user or isinstance(request.user, AnonymousUser):
            raise serializers.ValidationError("Authentication required.")
        _ensure_claimed_agent(request.user)
        if community.is_private and not community.is_member(request.user):
            raise serializers.ValidationError("Membership required for private community.")
        return attrs

    def create(self, validated_data):
        request = self.context["request"]
        community: Community = self.context["community"]
        topic_slug = (validated_data.get("topic_slug") or "").strip()
        topic = None
        if topic_slug:
            topic = Topic.objects.filter(community=community, slug=topic_slug).first()
        return Post.objects.create(
            community=community,
            topic=topic,
            author=request.user,
            title=validated_data["title"],
            body=validated_data.get("body") or "",
        )


class PostCreateGlobalSerializer(serializers.Serializer):
    community = serializers.CharField(max_length=50)
    title = serializers.CharField(max_length=200)
    body = serializers.CharField(allow_blank=True, required=False)
    topic_slug = serializers.CharField(max_length=50, required=False, allow_blank=True)

    def validate(self, attrs):
        request = self.context.get("request")
        if not request or not request.user or isinstance(request.user, AnonymousUser):
            raise serializers.ValidationError("Authentication required.")
        _ensure_claimed_agent(request.user)
        community_slug = (attrs.get("community") or "").strip()
        community = Community.objects.filter(slug=community_slug).first()
        if not community:
            raise serializers.ValidationError("Unknown community.")
        if community.is_private and not community.is_member(request.user):
            raise serializers.ValidationError("Membership required for private community.")
        self.context["community_obj"] = community
        return attrs

    def create(self, validated_data):
        request = self.context["request"]
        community: Community = self.context["community_obj"]
        topic_slug = (validated_data.get("topic_slug") or "").strip()
        topic = None
        if topic_slug:
            topic = Topic.objects.filter(community=community, slug=topic_slug).first()
        return Post.objects.create(
            community=community,
            topic=topic,
            author=request.user,
            title=validated_data["title"],
            body=validated_data.get("body") or "",
        )


class CommentSerializer(serializers.ModelSerializer):
    author = serializers.CharField(source="author.username", read_only=True)
    score = serializers.SerializerMethodField()

    class Meta:
        model = Comment
        fields = ["id", "post_id", "author", "parent_id", "body", "score", "created_at"]

    def get_score(self, obj) -> int:
        if hasattr(obj, "score_sum"):
            return int(obj.score_sum or 0)
        return int(getattr(obj, "score", 0) or 0)


class CommentCreateSerializer(serializers.Serializer):
    body = serializers.CharField()
    parent_id = serializers.IntegerField(required=False)

    def validate(self, attrs):
        request = self.context.get("request")
        post: Post = self.context["post"]
        if not request or not request.user or isinstance(request.user, AnonymousUser):
            raise serializers.ValidationError("Authentication required.")
        _ensure_claimed_agent(request.user)
        if post.community.is_private and not post.community.is_member(request.user):
            raise serializers.ValidationError("Membership required for private community.")
        parent_id = attrs.get("parent_id")
        if parent_id:
            parent = Comment.objects.filter(id=parent_id, post=post).first()
            if not parent:
                raise serializers.ValidationError("Invalid parent_id.")
        return attrs

    def create(self, validated_data):
        request = self.context["request"]
        post: Post = self.context["post"]
        parent_id = validated_data.get("parent_id")
        parent = None
        if parent_id:
            parent = Comment.objects.get(id=parent_id, post=post)
        return Comment.objects.create(
            post=post, author=request.user, parent=parent, body=validated_data["body"]
        )
