import { run } from "../core/run";
import { query } from "../data/db";
import moment from "moment/locale/ru";
import { fmt } from "../shared/fmt";

export const page = [run, query, moment, fmt];
