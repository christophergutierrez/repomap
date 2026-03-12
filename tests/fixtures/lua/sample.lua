-- Maximum number of retries
MAX_RETRIES = 3

-- Authenticate a token
function authenticate(token)
    return token ~= nil and #token > 0
end

-- User service module
local UserService = {}

function UserService.getUser(userId)
    return { id = userId, name = "test" }
end

return UserService
