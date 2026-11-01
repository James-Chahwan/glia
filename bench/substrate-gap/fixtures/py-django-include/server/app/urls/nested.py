from django.urls import path

urlpatterns = [
    path("things/", things),
]


def things(request):
    return []
