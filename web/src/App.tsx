import { useEffect, useState } from "react";
import { api, type Schemas } from "./api/client";

export function App() {
  const [bots, setBots] = useState<Schemas["BotSummary"][]>([]);

  useEffect(() => {
    void api.GET("/api/bots").then(({ data }) => setBots(data ?? []));
  }, []);

  return (
    <main>
      <h1>Buizz</h1>
      <ul>
        {bots.map((bot) => (
          <li key={bot.name}>
            {bot.name}: {bot.status}
          </li>
        ))}
      </ul>
    </main>
  );
}
