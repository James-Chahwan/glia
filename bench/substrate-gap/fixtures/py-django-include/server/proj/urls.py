from django.urls import include, path

urlpatterns = [
    path("api/", include("app.urls")),
    path("auth/", include("app.auth_urls")),
]
