const MAX_TIMEOUT = 5000;

/**
 * Base service class
 */
class BaseService {
    log(msg) {
        console.log(msg);
    }
}

/**
 * User service class
 */
class UserService extends BaseService {
    getUser(userId) {
        return { id: userId };
    }
}

function authenticate(token) {
    return token.length > 0;
}
