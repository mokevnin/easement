import type { Order } from "../../orders/model";

export const charge = (order: Order) => order.id;
