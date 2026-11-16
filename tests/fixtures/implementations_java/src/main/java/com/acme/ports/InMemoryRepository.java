package com.acme.ports;

public class InMemoryRepository implements OrderRepository {
    public String find() { return "memory"; }
}
