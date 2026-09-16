FROM python:3.12-slim
ENV DB_URL=postgres://db.internal:5432/app
COPY . /srv/app
CMD ["python", "-m", "app"]
