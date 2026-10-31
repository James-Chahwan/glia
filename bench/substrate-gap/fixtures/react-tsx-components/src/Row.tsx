import { type ReactNode } from 'react';
import { Child } from './Child';
import { ListItem } from './ui';

type RowProps = {
  id: string;
  title: ReactNode;
  isEditing?: boolean;
  onClick: () => void;
};

export const Row = ({
  id,
  title,
  isEditing = false,
  onClick,
}: RowProps) => {
  return (
    <ListItem
      render={
        <div
          data-id={id}
          onKeyDown={(event) => {
            if (event.key !== ' ') {
              return;
            }
            event.preventDefault();
          }}
          onClick={isEditing ? undefined : onClick}
        />
      }
      description={
        <>
          <Child title={`${id}`} />
        </>
      }
    >
      {title}
    </ListItem>
  );
};
