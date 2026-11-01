FROM python:3.12-slim
WORKDIR /code
COPY . .
RUN pip install -r requirements.txt
CMD ["./bin/docker-entrypoint-api.sh"]
