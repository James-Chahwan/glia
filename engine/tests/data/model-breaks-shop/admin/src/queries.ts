import { gql } from '@apollo/client/core';

export const STOCK_LIST = gql`
  query StockList($first: Int) {
    products(first: $first) {
      id
      name
      priceCents
      stock
      sku
    }
  }
`;

export const RESTOCK = gql`
  mutation Restock($id: ID!, $qty: Int!) {
    restock(id: $id, qty: $qty) {
      id
      stock
    }
  }
`;
