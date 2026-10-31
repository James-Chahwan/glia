FROM node:20-alpine
WORKDIR /app/server
COPY . .
RUN yarn install && yarn build
CMD ["node", "dist/main"]
