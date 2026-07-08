"""SQLAlchemy model + query (ACCESSES_DATA)."""
from sqlalchemy import Column, Integer, String
from sqlalchemy.orm import declarative_base, Session

Base = declarative_base()


class User(Base):
    __tablename__ = "users"
    id = Column(Integer, primary_key=True)
    name = Column(String)


def find_users(session: Session):
    return session.query(User).filter(User.name == "x").all()
