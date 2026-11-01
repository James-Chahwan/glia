import { Child } from './Child';
import { Row } from './Row';
import * as Icons from './icons';

function handle(event: unknown) {
  console.log(event);
}

export function Btn({ items }: { items: string[] }) {
  return (
    <div className="btn">
      <Child />
      <Icons.Plus size={12} />
      <button onClick={handle} />
      <List items={items} render={Row} />
    </div>
  );
}

function List({ items, render }: { items: string[]; render: unknown }) {
  return <ul>{items.length}</ul>;
}

// A lowercase JSX tag is an intrinsic element, never this function.
function div(): string {
  return 'div';
}
