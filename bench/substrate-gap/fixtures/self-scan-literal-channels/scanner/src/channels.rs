//! A scanner's own fixtures: the browser `new WebSocket(url)` client, the
//! NestJS `@WebSocketGateway` handler and the Go `pb.NewOrderServiceClient(conn)`
//! stub, as the WebSocket, gRPC and GraphQL scanners see them.

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_literals() {
        let ws = "const ws = new WebSocket('ws://localhost:8080/chat');";
        let grpc = "conn := grpc.Dial(addr)\nclient := pb.NewOrderServiceClient(conn)";
        let gql = "import { useQuery } from '@apollo/client';\nconst { data } = useQuery(GET_USERS);";
        assert!(!ws.is_empty() && !grpc.is_empty() && !gql.is_empty());
    }
}
