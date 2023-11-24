export const isRFC3339DateTime = (str: string) => {
    const rfc3339Pattern = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[-+]\d{2}:\d{2}))$/;
  
    return rfc3339Pattern.test(str);
  }