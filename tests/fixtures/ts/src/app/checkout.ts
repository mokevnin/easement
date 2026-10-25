import { z } from "zod";
import type { Order } from "../domain/order";
import { save } from "@/infra/db";
import { id } from "./util.js";

export const checkout = (): Order => save({ id: id() });
