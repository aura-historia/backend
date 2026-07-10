function handler(event) {
    var request = event.request;
    var headers = request.headers;
    var authHeader = headers.authorization;

    if (authHeader && authHeader.value) {
        var value = authHeader.value;

        if (value.startsWith("Bearer ")) {
            var token = value.substring(7);

            if (token.split(".").length === 3) {
                if (!request.querystring) {
                    request.querystring = {};
                }

                request.querystring["__auth"] = { value: "1" };
            }
        }
    }

    return request;
}
