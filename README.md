# Fatebook Beeminder Bridge

How to deploy:

```bash
# 1. Make sure you have a .env file in the daemo-engine directory
# (Same .env you created earlier with MongoDB URI, API keys, etc.)

# 2. Build and start the services
docker-compose up --build

# 3. Run in detached mode (background)
docker-compose up -d

# 4. View logs
docker-compose logs -f daemo-engine

# 5. Check health
curl http://localhost:8080/health

# 6. Test gRPC (you'll need grpcurl installed)
# Install grpcurl: brew install grpcurl (Mac) or download from GitHub
grpcurl -plaintext localhost:50052 list

# 7. Stop services
docker-compose down

# 8. Rebuild after code changes
docker-compose up --build --force-recreate

# 9. Clean everything (including volumes)
docker-compose down -v

# 10. Execute commands inside the running container
docker-compose exec daemo-engine /bin/bash
```
