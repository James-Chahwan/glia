"""An admin script over a generated Python SDK: `call_api(path, method=..)`."""


def remove_activity(api, activity_id):
    return api.call_api(f"/activities/{activity_id}", method="DELETE")
