import { query } from "../data/db";
import { page } from "../web/page";
import { fmt } from "../shared/fmt";
import _ from "lodash/fp";
import axios from "axios";

export const run = [query, page, fmt, _, axios];
