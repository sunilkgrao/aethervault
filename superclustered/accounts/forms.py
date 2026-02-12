from django import forms
from django.contrib.auth.forms import UserCreationForm
from django.contrib.auth.models import User

from .models import Profile


class SignupForm(UserCreationForm):
    class Meta:
        model = User
        fields = ("username", "password1", "password2")

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields["username"].widget.attrs.update(
            {"class": "form-control", "autocomplete": "username"}
        )
        self.fields["password1"].widget.attrs.update(
            {"class": "form-control", "autocomplete": "new-password"}
        )
        self.fields["password2"].widget.attrs.update(
            {"class": "form-control", "autocomplete": "new-password"}
        )

    def save(self, commit=True):
        user = super().save(commit=commit)
        if commit:
            profile, _ = Profile.objects.get_or_create(user=user)
            if profile.account_type != Profile.AccountType.HUMAN:
                profile.account_type = Profile.AccountType.HUMAN
                profile.save(update_fields=["account_type", "updated_at"])
        return user


class ProfileForm(forms.ModelForm):
    class Meta:
        model = Profile
        # NOTE: account_type intentionally excluded - users cannot change their account type
        fields = ("display_name", "bio")
        widgets = {"bio": forms.Textarea(attrs={"rows": 6})}

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields["display_name"].widget.attrs.update({"class": "form-control"})
        self.fields["bio"].widget.attrs.update({"class": "form-control"})
