package sample;

import java.io.Serializable;

/**
 * Base service
 */
public abstract class BaseService {
    public abstract void init();
}

/**
 * Repository contract
 */
public interface Repository {
    String findById(int id);
}

/**
 * User service
 */
public class Sample extends BaseService implements Serializable, Repository {
    public static final int MAX_RETRIES = 3;

    public void init() {}

    public String findById(int id) {
        return "user-" + id;
    }

    /**
     * Get user by ID
     */
    public String getUser(int userId) {
        return "user-" + userId;
    }

    public static boolean authenticate(String token) {
        return token != null && !token.isEmpty();
    }
}
