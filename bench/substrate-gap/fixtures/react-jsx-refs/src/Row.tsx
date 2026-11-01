export const Row = ({
  label,
  onSelect,
}: {
  label: string;
  onSelect: () => void;
}) => {
  return (
    <li onClick={onSelect}>
      {label}
    </li>
  );
};
