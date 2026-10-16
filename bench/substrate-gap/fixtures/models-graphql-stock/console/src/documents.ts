import { gql } from '@apollo/client';

export const LIST_ITEMS = gql`
  query ListItems($first: Int) {
    items(first: $first) {
      id
      label
      priceCents
      onHand
      barcode
    }
  }
`;

export const ADJUST_STOCK = gql`
  mutation AdjustStock($id: ID!, $delta: Int!) {
    adjustStock(id: $id, delta: $delta) {
      id
      onHand
    }
  }
`;
