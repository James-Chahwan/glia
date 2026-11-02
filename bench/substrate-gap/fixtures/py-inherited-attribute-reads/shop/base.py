class BaseController:
    def __init__(self, session):
        self.session = session
        self._repos = None

    @property
    def repos(self):
        if self._repos is None:
            self._repos = {"orders": [], "users": []}
        return self._repos
