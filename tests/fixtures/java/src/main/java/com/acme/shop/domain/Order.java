package com.acme.shop.domain;

public record Order(String id) {
    public static Order empty() { return new Order(""); }
}
