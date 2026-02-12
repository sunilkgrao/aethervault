from django import forms

from communities.models import Topic

from .models import Comment, Post


class MultiFileInput(forms.ClearableFileInput):
    allow_multiple_selected = True


class PostForm(forms.ModelForm):
    attachments = forms.FileField(
        required=False, widget=MultiFileInput(attrs={"multiple": True})
    )

    class Meta:
        model = Post
        fields = ("title", "topic", "body")
        widgets = {"body": forms.Textarea(attrs={"rows": 10})}

    def __init__(self, *args, community=None, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields["title"].widget.attrs.update({"class": "form-control"})
        self.fields["body"].widget.attrs.update({"class": "form-control"})
        if "topic" in self.fields:
            self.fields["topic"].widget.attrs.update({"class": "form-select"})
        if "topic" in self.fields:
            if community is None:
                self.fields["topic"].queryset = Topic.objects.none()
            else:
                self.fields["topic"].queryset = Topic.objects.filter(community=community)
            self.fields["topic"].required = False
        self.fields["attachments"].widget.attrs.update({"class": "form-control"})


class CommentForm(forms.ModelForm):
    attachments = forms.FileField(
        required=False, widget=MultiFileInput(attrs={"multiple": True})
    )
    parent_id = forms.IntegerField(required=False, widget=forms.HiddenInput())

    class Meta:
        model = Comment
        fields = ("body",)
        widgets = {"body": forms.Textarea(attrs={"rows": 4})}

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.fields["body"].widget.attrs.update({"class": "form-control"})
        self.fields["attachments"].widget.attrs.update({"class": "form-control"})
