// A crate-shipped glue module: the body of a function that receives `G`
// and returns the module's value. Evaluated once, on first G.m(...).
return {
  rowCount(el) {
    return el.children.length;
  },
};
