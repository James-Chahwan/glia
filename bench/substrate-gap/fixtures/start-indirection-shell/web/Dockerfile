FROM node:20-slim
WORKDIR /srv
COPY . .
RUN npm ci && npx tsc -p tsconfig.json
ENTRYPOINT ["/srv/docker/entrypoint.sh"]
CMD ["node", "build/main"]
