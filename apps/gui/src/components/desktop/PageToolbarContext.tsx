import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

interface PageToolbarContextValue {
  toolbar: ReactNode | null;
  setToolbar: (toolbar: ReactNode | null) => void;
}

const PageToolbarContext = createContext<PageToolbarContextValue | null>(null);

export function PageToolbarProvider({ children }: { children: ReactNode }) {
  const [toolbar, setToolbar] = useState<ReactNode | null>(null);
  const value = useMemo(() => ({ toolbar, setToolbar }), [toolbar]);

  return (
    <PageToolbarContext.Provider value={value}>
      {children}
    </PageToolbarContext.Provider>
  );
}

export function usePageToolbar() {
  return useContext(PageToolbarContext);
}

export function useRegisterPageToolbar(toolbar: ReactNode | null) {
  const setToolbar = usePageToolbar()?.setToolbar;

  useEffect(() => {
    if (!setToolbar) {
      return;
    }

    setToolbar(toolbar);
    return () => {
      setToolbar(null);
    };
  }, [setToolbar, toolbar]);
}
