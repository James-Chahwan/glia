from django.urls import path

urlpatterns = [
    path("workspaces/<str:slug>/issues/", issue_list),
]


def issue_list(request, slug):
    return []
