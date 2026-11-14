# Static build of the public website (site/). Build from the site folder:
#   docker build -f ../infra/site.Dockerfile --build-arg VITE_DASHBOARD_URL=https://app.example.com -t axom-site site
FROM node:22.23.3-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY . .
ARG VITE_DASHBOARD_URL
ARG VITE_JOBS_URL
ARG VITE_BRIEFS_URL
ARG VITE_REPO_URL
ARG VITE_PLAN_PRICE_USD
ARG VITE_TRIAL_DAYS
RUN npm run build

FROM nginx:1.31.7-alpine
COPY --from=build /app/dist /usr/share/nginx/html
# A multi-page site, not a single-page app: /pricing/ serves pricing/index.html
# and unknown paths get the site's own 404 page. Compressed, and the hashed
# files in /assets/ are cached for a year (a new build gets new names).
RUN printf 'server {\n  listen 80;\n  root /usr/share/nginx/html;\n  gzip on;\n  gzip_comp_level 6;\n  gzip_types text/css text/javascript application/javascript image/svg+xml;\n  location / { try_files $uri $uri/index.html =404; }\n  location /assets/ { expires 1y; }\n  location /fonts/ { expires 30d; }\n  error_page 404 /404.html;\n  add_header X-Frame-Options DENY;\n  add_header X-Content-Type-Options nosniff;\n  add_header Referrer-Policy strict-origin-when-cross-origin;\n}\n' > /etc/nginx/conf.d/default.conf
EXPOSE 80
