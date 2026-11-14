# syntax=docker/dockerfile:1
# Static build for admin/ or meet-web/. Build from the app folder (BuildKit):
#   docker build -f ../infra/web.Dockerfile --build-arg VITE_AXOM_API_URL=https://api.example.com -t axom-admin admin
FROM node:22.23.3-alpine AS build
WORKDIR /app
# Dependencies first: this layer is reused until the lockfile changes, and the
# npm cache mount makes a lockfile change download only what is new.
COPY package.json package-lock.json ./
RUN --mount=type=cache,id=combox-npm,target=/root/.npm \
    npm ci --no-audit --no-fund --prefer-offline
COPY . .
ARG VITE_AXOM_API_URL
ARG VITE_SUPABASE_URL
ARG VITE_SUPABASE_ANON_KEY
ARG VITE_AXOM_MEET_WEB_URL
ARG VITE_JUDGE_MODE
ARG VITE_SESSION_COOKIE_DOMAIN
RUN npm run build

FROM nginx:1.31.7-alpine
COPY --from=build /app/dist /usr/share/nginx/html
# Single-page app: unknown paths serve index.html (e.g. /join?t=...).
RUN printf 'server {\n  listen 80;\n  root /usr/share/nginx/html;\n  location / { try_files $uri /index.html; }\n  add_header X-Frame-Options DENY;\n  add_header X-Content-Type-Options nosniff;\n  add_header Referrer-Policy no-referrer;\n}\n' > /etc/nginx/conf.d/default.conf
EXPOSE 80
