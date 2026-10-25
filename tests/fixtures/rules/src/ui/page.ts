import { list } from "../orders/service";
// easement:disable-next-line dependency -- migrating to the orders API, see #12
import { charge } from "../billing";
import { charge as raw } from "../billing/internal/charge"; // easement:disable-line dependency
// easement:disable-next-line privacy -- stale exception
export const page = () => [list(), charge, raw];
