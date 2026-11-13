import { ResponseOutparam as __jco_ResponseOutparam, OutgoingResponse as __jco_OutgoingResponse, OutgoingBody as __jco_OutgoingBody, Fields as __jco_Fields, IncomingRequest as __jco_IncomingRequest } from 'wasi:http/types';

interface __jco_FetchEvent { request: Request }

const __jco_fetch_headers = ['content-type', 'content-encoding', 'content-language', 'content-disposition', 'content-location', 'content-range', 'content-security-policy', 'cache-control', 'location', 'set-cookie', 'etag', 'last-modified', 'expires', 'vary', 'allow', 'link', 'retry-after', 'www-authenticate', 'accept-ranges', 'access-control-allow-origin', 'access-control-allow-methods', 'access-control-allow-headers', 'access-control-allow-credentials', 'access-control-expose-headers', 'access-control-max-age', 'strict-transport-security', 'x-content-type-options', 'x-frame-options'];

function __jco_fetch_send(out: __jco_ResponseOutparam, status: number, headers: __jco_Fields, bytes: Uint8Array): void {
  const response = new __jco_OutgoingResponse(headers);
  response.setStatusCode(status);
  const body = response.body();
  __jco_ResponseOutparam.set(out, { tag: 'ok', val: response });
  const stream = body.write();
  let offset = 0;
  while (offset < bytes.length) {
    const end = Math.min(offset + 4096, bytes.length);
    stream.blockingWriteAndFlush(bytes.subarray(offset, end));
    offset = end;
  }
  stream[Symbol.dispose]();
  __jco_OutgoingBody.finish(body, null);
}

async function __jco_fetch_answer(request: __jco_IncomingRequest): Promise<Response> {
  const method = request.method();
  const scheme = request.scheme();
  const authority = request.authority();
  const path = request.pathWithQuery();
  const url = (scheme === null ? 'http' : scheme.tag === 'other' ? scheme.val : scheme.tag.toLowerCase())
    + '://' + (authority === null ? 'localhost' : authority) + (path === null ? '/' : path);
  const verb = method.tag === 'other' ? method.val : method.tag.toUpperCase();
  const headers = new Headers();
  const decoder = new TextDecoder();
  for (const entry of request.headers().entries()) {
    headers.append(entry[0], decoder.decode(entry[1]));
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  const incoming = request.consume();
  const input = incoming.stream();
  try {
    while (true) {
      const chunk = input.blockingRead(65536n);
      chunks.push(chunk);
      total += chunk.length;
    }
  } catch (end) {
    total += 0;
  }
  input[Symbol.dispose]();
  incoming[Symbol.dispose]();
  if (total === 0) {
    return __jco_fetch_listener({ request: new Request(url, { method: verb, headers: headers }) });
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.length;
  }
  return __jco_fetch_listener({ request: new Request(url, { method: verb, headers: headers, body: body }) });
}

export const incomingHandler = {
  async handle(request: __jco_IncomingRequest, out: __jco_ResponseOutparam): Promise<void> {
    try {
      const encoder = new TextEncoder();
      const headers = new __jco_Fields();
      let status = 500;
      let bytes = encoder.encode('Internal Server Error');
      try {
        const response = await __jco_fetch_answer(request);
        const body = await response.bytes();
        for (const name of __jco_fetch_headers) {
          const value = response.headers.get(name);
          if (value !== null) {
            headers.append(name, encoder.encode(value));
          }
        }
        status = response.status;
        bytes = body;
      } catch (error) {
        status = 500;
      }
      __jco_fetch_send(out, status, headers, bytes);
    } catch (error) {}
  },
};
