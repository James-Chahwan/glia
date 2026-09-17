import Vapor

func getUser(req: Request) async throws -> String {
    let id = req.parameters.get("id") ?? ""
    return "user \(id)"
}

func createUser(req: Request) async throws -> HTTPStatus {
    return .created
}

func routes(_ app: Application) throws {
    app.get("users", ":id", use: getUser)
    app.post("users", use: createUser)
}
