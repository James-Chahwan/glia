from django.urls import include, path

from .issue import urlpatterns as issue_urls

urlpatterns = [
    *issue_urls,
    path("v2/", include("app.urls.nested")),
]
