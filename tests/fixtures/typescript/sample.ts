const MAX_TIMEOUT: number = 5000;

interface User {
    id: number;
    name: string;
}

interface Searchable {
    search(query: string): User[];
}

class BaseService {
    protected log(msg: string): void {}
}

class UserService extends BaseService implements Searchable {
    getUser(userId: number): User {
        return { id: userId, name: "" };
    }

    search(query: string): User[] {
        return [];
    }
}

function authenticate(token: string): boolean {
    return token.length > 0;
}

type UserID = number;
