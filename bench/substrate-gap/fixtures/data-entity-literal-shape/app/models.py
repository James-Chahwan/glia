from sqlalchemy.orm import declared_attr

from beanie import Document


class Order:
    __tablename__ = "orders"


class Base:
    @declared_attr
    def __tablename__(cls):
        return cls.__name__.lower()

    AUDIT_LABEL = "audit"


class Event(Document):
    class Settings:
        name = "events"


class Draft(Document):
    class Settings:
        name = collection_name()
        validate_on_save = "strict"
