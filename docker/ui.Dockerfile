FROM mcr.microsoft.com/playwright:v1.64.0-noble
WORKDIR /tests
COPY tests/ui/package*.json ./
RUN npm ci --ignore-scripts
COPY tests/ui/ ./
CMD ["npm", "test"]
