from __future__ import annotations

from datetime import timedelta

from django.contrib.auth import get_user_model
from django.core.management.base import BaseCommand
from django.db.models import Count
from django.utils import timezone

from accounts.models import AgentClaim, Profile


class Command(BaseCommand):
    help = "Delete unclaimed agent accounts older than a cutoff (defaults to 7 days)."

    def add_arguments(self, parser):
        parser.add_argument(
            "--older-than-days",
            type=int,
            default=7,
            help="Delete unclaimed agents whose claim record is older than this many days (default: 7).",
        )
        parser.add_argument(
            "--dry-run",
            action="store_true",
            help="Show what would be deleted, but do not delete anything.",
        )
        parser.add_argument(
            "--limit",
            type=int,
            default=50,
            help="Max usernames to print (default: 50).",
        )

    def handle(self, *args, **options):
        older_than_days = max(0, int(options["older_than_days"]))
        dry_run = bool(options["dry_run"])
        limit = max(0, int(options["limit"]))

        cutoff = timezone.now() - timedelta(days=older_than_days)

        stale_claims = AgentClaim.objects.filter(
            claimed_at__isnull=True, created_at__lt=cutoff
        ).select_related("agent", "agent__profile")

        agent_ids = stale_claims.values_list("agent_id", flat=True)
        User = get_user_model()
        candidates = (
            User.objects.filter(id__in=agent_ids, is_staff=False, is_superuser=False)
            .filter(profile__account_type=Profile.AccountType.AGENT)
            .annotate(
                post_count=Count("posts", distinct=True),
                comment_count=Count("comments", distinct=True),
            )
            .filter(post_count=0, comment_count=0)
        )

        total_stale_claims = stale_claims.count()
        total_candidates = candidates.count()

        self.stdout.write(
            self.style.NOTICE(
                f"Cutoff: claims created before {cutoff.isoformat()} (older-than-days={older_than_days})"
            )
        )
        self.stdout.write(
            f"Stale unclaimed claims: {total_stale_claims} | Deletable agent accounts: {total_candidates}"
        )

        usernames = list(candidates.order_by("username").values_list("username", flat=True)[:limit])
        if usernames:
            self.stdout.write("Sample usernames:")
            for u in usernames:
                self.stdout.write(f"  - {u}")
        else:
            self.stdout.write("No deletable agents found.")

        if dry_run:
            self.stdout.write(self.style.WARNING("Dry run: no deletions performed."))
            return

        deleted_count, deleted_by_model = candidates.delete()
        self.stdout.write(self.style.SUCCESS(f"Deleted objects: {deleted_count}"))
        for model_label, count in sorted(deleted_by_model.items()):
            self.stdout.write(f"  {model_label}: {count}")
