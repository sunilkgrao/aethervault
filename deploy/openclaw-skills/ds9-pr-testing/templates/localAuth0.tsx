import React, { createContext, useContext } from 'react';

export type User = {
  email?: string;
  name?: string;
  nickname?: string;
  picture?: string;
  sub?: string;
  [key: string]: any;
};

export type LogoutOptions = Record<string, any>;

type Auth0ContextValue = {
  isAuthenticated: boolean;
  isLoading: boolean;
  error?: Error;
  user: User;
  loginWithRedirect: (...args: any[]) => Promise<void>;
  logout: (...args: any[]) => void;
  getAccessTokenSilently: (...args: any[]) => Promise<string>;
  getAccessTokenWithPopup: (...args: any[]) => Promise<string>;
  handleRedirectCallback: (...args: any[]) => Promise<{ appState: any }>;
};

const normalizeEmail = (email: string) => email.replace(/@/g, '__at__');

const email =
  import.meta.env.VITE_LOCAL_DEV_AUTH_EMAIL || 'sunil@tribble.ai';
const token =
  import.meta.env.VITE_LOCAL_DEV_AUTH_TOKEN ||
  `local-dev-token.${normalizeEmail(email)}`;

const authValue: Auth0ContextValue = {
  isAuthenticated: true,
  isLoading: false,
  error: undefined,
  user: {
    email,
    name: email.split('@')[0],
    nickname: email.split('@')[0],
    sub: `local-dev|${email}`,
  },
  loginWithRedirect: async () => {},
  logout: () => {
    window.location.reload();
  },
  getAccessTokenSilently: async () => token,
  getAccessTokenWithPopup: async () => token,
  handleRedirectCallback: async () => ({ appState: undefined }),
};

const Auth0Context = createContext<Auth0ContextValue>(authValue);

export function Auth0Provider({ children }: { children: React.ReactNode }) {
  return (
    <Auth0Context.Provider value={authValue}>{children}</Auth0Context.Provider>
  );
}

export function useAuth0() {
  return useContext(Auth0Context);
}

export function withAuthenticationRequired(
  component: React.ComponentType,
  _options?: any,
) {
  return component;
}
