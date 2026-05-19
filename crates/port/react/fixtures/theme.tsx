import { createContext, useContext } from "react";

interface ThemeContext {
    accent: string;
}

const Theme = createContext<ThemeContext | null>(null);

interface ThemedProps {
    label: string;
}

export function Themed({ label }: ThemedProps) {
    const theme = useContext(Theme);
    return (
        <View>
            <Text>{label}: {theme.accent}</Text>
        </View>
    );
}

interface AppProps {
    accent: string;
}

export function App({ accent }: AppProps) {
    return (
        <Theme.Provider value={{ accent }}>
            <Themed label="hello" />
        </Theme.Provider>
    );
}
