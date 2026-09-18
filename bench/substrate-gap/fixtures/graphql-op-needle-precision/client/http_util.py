import requests


class Api:
    def handle_request(self, request):
        return requests.request("GET", request.url)
