FROM python:3.12-slim
WORKDIR /opt/tool
COPY . .
RUN pip install .
ENTRYPOINT ["/opt/tool/entry.sh"]
