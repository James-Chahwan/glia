import { forwardRef, memo } from 'react';

function format(title: string) {
  return title.trim();
}

export const Card = memo(({ title }: { title: string }) => {
  return <div>{format(title)}</div>;
});

export const Field = forwardRef<HTMLInputElement, { name: string }>(
  (props, ref) => <input ref={ref} name={format(props.name)} />,
);
