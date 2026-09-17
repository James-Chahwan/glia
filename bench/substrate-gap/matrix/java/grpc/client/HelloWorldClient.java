package com.example.demo;

import io.grpc.Channel;

public class HelloWorldClient {
    private final GreeterGrpc.GreeterBlockingStub blockingStub;

    public HelloWorldClient(Channel channel) {
        blockingStub = GreeterGrpc.newBlockingStub(channel);
    }

    public String greet(String name) {
        HelloRequest request = HelloRequest.newBuilder().setName(name).build();
        return blockingStub.sayHello(request).getMessage();
    }
}
