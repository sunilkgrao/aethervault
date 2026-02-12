from django.urls import path

from . import views

urlpatterns = [
    path("<uuid:attachment_id>/download/", views.download_attachment, name="attachment-download"),
]

