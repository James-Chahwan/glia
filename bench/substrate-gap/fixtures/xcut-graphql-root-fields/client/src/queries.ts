import { gql } from '@apollo/client/core';

export const STOCK_LIST = gql`
  query StockList($first: Int) {
    products(first: $first) { id name }
  }
`;

export const DASHBOARD = gql`
  query Dashboard {
    me { id }
    recent: orders(last: 5) { id }
  }
`;

export const RESTOCK = gql`
  mutation Restock($id: ID!) {
    restock(id: $id) { id }
  }
`;

export const PRODUCTS = gql`
  query Products {
    __typename
    me { id }
  }
`;

export const RECENT = gql`
  {
    orders(last: 1) { id }
  }
`;
