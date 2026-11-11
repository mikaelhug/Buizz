import { useState } from "react";
import { api, ApiFailure, errorMessage, mustSucceed } from "../api/client";

export function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await mustSucceed(api.POST("/api/auth/login", { body: { password } }));
      onLoggedIn();
    } catch (failure) {
      setError(failure instanceof ApiFailure && failure.status === 401 ? "Wrong password" : errorMessage(failure));
      setBusy(false);
    }
  };

  return (
    <main className="login">
      <form
        className="login-card"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <h1 className="brand">Buizz</h1>
        <input
          className="input"
          type="password"
          autoComplete="current-password"
          placeholder="Password"
          aria-label="Password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          autoFocus
          required
        />
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <button type="submit" className="btn btn-accent" disabled={busy || password.length === 0}>
          Sign in
        </button>
      </form>
    </main>
  );
}
