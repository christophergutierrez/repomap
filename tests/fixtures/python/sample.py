MAX_RETRIES = 3

class BaseService:
    """Base service class."""
    def health_check(self) -> bool:
        """Check service health."""
        return True

class UserService(BaseService):
    """Manages user operations."""
    def get_user(self, user_id: int) -> dict:
        """Get user by ID."""
        return {"id": user_id}

    def delete_user(self, user_id: int) -> bool:
        """Delete a user."""
        return True

def authenticate(token: str) -> bool:
    """Authenticate a token."""
    return len(token) > 0
