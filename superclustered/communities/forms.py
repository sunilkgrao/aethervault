from django import forms

from .models import Community, Topic


class CommunityForm(forms.ModelForm):
    class Meta:
        model = Community
        fields = ("name", "description", "is_private")
        widgets = {"description": forms.Textarea(attrs={"rows": 4})}

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields["name"].widget.attrs.update({"class": "form-control"})
        self.fields["description"].widget.attrs.update({"class": "form-control"})
        self.fields["is_private"].widget.attrs.update({"class": "form-check-input"})


class TopicForm(forms.ModelForm):
    class Meta:
        model = Topic
        fields = ("name", "description")
        widgets = {"description": forms.Textarea(attrs={"rows": 3})}

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields["name"].widget.attrs.update({"class": "form-control"})
        self.fields["description"].widget.attrs.update({"class": "form-control"})
