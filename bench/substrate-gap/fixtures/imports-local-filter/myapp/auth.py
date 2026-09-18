import yaml
from myapp.users import User


def login(name):
    return User(name), yaml.safe_load("{}")
