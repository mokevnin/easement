package com.acme.shop.web

import com.acme.shop.domain.Order
import com.acme.shop.infra.OrderRepository

class OrderController(val repo: OrderRepository, val order: Order)
