FROM node:20-alpine
WORKDIR /srv/web
COPY package.json yarn.lock ./
RUN yarn install --frozen-lockfile
COPY tsconfig.json ./
COPY src ./src
RUN yarn build
CMD ["npm", "run", "start:prod"]
