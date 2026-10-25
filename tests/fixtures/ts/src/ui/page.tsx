import { checkout } from "../app/checkout";
const db = require("../infra/db");

export const Page = () => <button onClick={checkout}>{String(db)}</button>;
