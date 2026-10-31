import { useState } from 'react';
import { Button, IconPencil, TextInput, Tooltip } from './ui';

export const EmailField = ({ userId }: { userId: string }) => {
  const [isEditing, setIsEditing] = useState(false);

  const handleStartEditing = () => {
    setIsEditing(true);
  };

  const handleSave = async () => {
    setIsEditing(false);
  };

  return (
    <div>
      <TextInput
        instanceId={`user-email-${userId}`}
        onEnter={handleSave}
      />
      <Tooltip
        disabled={!isEditing}
      >
        <Button
          startIcon={<IconPencil />}
          onClick={handleStartEditing}
        />
      </Tooltip>
    </div>
  );
};
