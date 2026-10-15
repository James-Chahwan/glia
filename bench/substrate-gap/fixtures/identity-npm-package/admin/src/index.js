import _ from "lodash";

export function total(rows) {
  return _.sumBy(rows, "cents");
}
