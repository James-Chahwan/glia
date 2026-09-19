package com.example;

import lombok.RequiredArgsConstructor;

@Service
class OrderService { public String place() { return "ok"; } }

@Repository
class OrderRepo { }

@RestController
@RequiredArgsConstructor
class OrderController {
    private final OrderService orderService;
    private final OrderRepo orderRepo;
    private final String greeting = "hi";
    private static final int MAX = 5;
}
