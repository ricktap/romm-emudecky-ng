// Vite's `?raw` import: the file's text (used by tests that read a source file of the backend).
declare module "*?raw" {
  const content: string;
  export default content;
}
