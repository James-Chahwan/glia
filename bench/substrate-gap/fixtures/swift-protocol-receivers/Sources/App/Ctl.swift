import Vapor

struct Ctl: RouteCollection {
    func boot(routes: RoutesBuilder) throws {
        routes.get("items", use: self.index)
    }

    func index(req: Request) -> String {
        ""
    }
}
