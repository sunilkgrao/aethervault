from django.urls import path

from . import views

urlpatterns = [
    path("", views.home, name="home"),
    path("mission/", views.mission, name="mission"),
    path("rules/", views.rules, name="rules"),
    path("skill.md", views.skill_md, name="skill-md"),
    path("healthz/", views.healthz, name="healthz"),
]
