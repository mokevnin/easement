package com.acme.shop.infra

import com.acme.shop.domain.Order
import kotlin.collections.List

class OrderRepository(val orders: List<Order>)
