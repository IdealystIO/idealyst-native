import { useState, useEffect } from "react";

interface TodoItemProps {
    id: string;
    title: string;
    completed: boolean;
    onToggle: (id: string) => void;
    onDelete: (id: string) => void;
}

export function TodoItem({ id, title, completed, onToggle, onDelete }: TodoItemProps) {
    const [isEditing, setIsEditing] = useState(false);
    const [draft, setDraft] = useState(title);

    useEffect(() => {
        if (!isEditing) {
            setDraft(title);
        }
    }, [isEditing, title]);

    return (
        <View>
            {isEditing ? (
                <TextInput value={draft} onChange={(e) => setDraft(e.target.value)} />
            ) : (
                <Text>{title}</Text>
            )}
            <Button label={completed ? "Undo" : "Done"} onClick={() => onToggle(id)} />
            <Button label="Delete" onClick={() => onDelete(id)} />
        </View>
    );
}
