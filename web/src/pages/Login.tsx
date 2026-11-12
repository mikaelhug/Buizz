import { useState } from "react";
import { api, ApiFailure, errorMessage, mustSucceed } from "../api/client";
import { useDocumentTitle } from "../lib/route";
import { BrandMark } from "../ui/Icon";
import { Info } from "../ui/Popover";

export function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  useDocumentTitle("Sign in");

  const submit = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await mustSucceed(api.POST("/api/auth/login", { body: { username, password } }));
      onLoggedIn();
    } catch (failure) {
      setError(failure instanceof ApiFailure && failure.status === 401 ? "Wrong username or password" : errorMessage(failure));
      setBusy(false);
    }
  };

  return (
    <main className="login">
      <form
        method="post"
        className="login-card"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <h1 className="brand">
          <BrandMark />
          Buizz
        </h1>
        <label className="login-field">
          Username
          <input
            className="input"
            type="text"
            name="username"
            autoComplete="username"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            autoFocus
            required
          />
        </label>
        <label className="login-field">
          Password
          <input
            className="input"
            type="password"
            name="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
          />
        </label>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <button type="submit" className="btn btn-primary" disabled={busy || username.length === 0 || password.length === 0}>
          Sign in
        </button>
      </form>
    </main>
  );
}

export function Unreachable({ reason, onRetry }: { reason: string; onRetry: () => void }) {
  useDocumentTitle("Server unreachable");
  return (
    <main className="login">
      <div className="login-card" role="alert">
        <h1 className="brand">
          <BrandMark />
          Buizz
        </h1>
        <p className="unreachable">
          <span className="stream-dot stream-down" aria-hidden="true" />
          Server unreachable
          <Info label="Server unreachable">{reason}</Info>
        </p>
        <button type="button" className="btn" onClick={onRetry}>
          Retry
        </button>
      </div>
    </main>
  );
}
