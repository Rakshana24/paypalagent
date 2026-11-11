import type {
  HealthResponse,
  AuthCheckResponse,
  DisputeEvidenceBrief,
  ApiErrorResponse,
  ShoppingSessionDetail,
  DecisionStep,
  Product,
  ShoppingSessionResponse,
  AuthorizePurchaseRequest,
  AuthorizePurchaseResponse,
  CreateOrderResponse,
  CaptureOrderResponse,
  PayPalOrderStatusResponse,
  VerificationResult,
} from '../types/api';

export class ApiClientError extends Error {
  status: number;
  data?: ApiErrorResponse | unknown;

  constructor(message: string, status: number, data?: unknown) {
    super(message);
    this.name = 'ApiClientError';
    this.status = status;
    this.data = data;
  }
}

async function handleResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let errorData: unknown;
    try {
      errorData = await res.json();
    } catch {
      // response might be plain text or HTML
    }
    const message =
      (errorData as ApiErrorResponse)?.message ||
      `Request failed with status ${res.status}`;
    throw new ApiClientError(message, res.status, errorData);
  }
  return res.json() as Promise<T>;
}

export async function fetchHealth(): Promise<HealthResponse> {
  const res = await fetch('/health');
  return handleResponse<HealthResponse>(res);
}

export async function fetchPaypalAuthCheck(): Promise<AuthCheckResponse> {
  const res = await fetch('/api/paypal/auth-check');
  return handleResponse<AuthCheckResponse>(res);
}

export async function fetchDisputeEvidence(
  disputeId: string
): Promise<DisputeEvidenceBrief> {
  const trimmed = disputeId.trim();
  if (!trimmed) {
    throw new ApiClientError('Dispute ID cannot be empty', 400);
  }
  const res = await fetch(`/api/disputes/${encodeURIComponent(trimmed)}/evidence`);
  return handleResponse<DisputeEvidenceBrief>(res);
}

export async function fetchDisputeEvidenceMarkdown(
  disputeId: string
): Promise<string> {
  const trimmed = disputeId.trim();
  if (!trimmed) {
    throw new ApiClientError('Dispute ID cannot be empty', 400);
  }
  const res = await fetch(
    `/api/disputes/${encodeURIComponent(trimmed)}/evidence/markdown`
  );
  if (!res.ok) {
    let errorData: unknown;
    try {
      errorData = await res.json();
    } catch {
      // ignore
    }
    const message =
      (errorData as ApiErrorResponse)?.message ||
      `Request failed with status ${res.status}`;
    throw new ApiClientError(message, res.status, errorData);
  }
  return res.text();
}

export async function fetchShoppingSession(
  sessionId: string
): Promise<ShoppingSessionDetail> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(`/api/shopping/sessions/${encodeURIComponent(trimmed)}`);
  return handleResponse<ShoppingSessionDetail>(res);
}

export async function fetchSessionEvents(
  sessionId: string
): Promise<DecisionStep[]> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(`/api/sessions/${encodeURIComponent(trimmed)}/events`);
  return handleResponse<DecisionStep[]>(res);
}

export async function fetchCatalogProducts(): Promise<Product[]> {
  const res = await fetch('/api/catalog/products');
  return handleResponse<Product[]>(res);
}

export async function createShoppingSession(
  prompt: string
): Promise<ShoppingSessionResponse> {
  const trimmed = prompt.trim();
  if (!trimmed) {
    throw new ApiClientError('Shopping prompt cannot be empty', 400);
  }
  const res = await fetch('/api/shopping/sessions', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ prompt: trimmed }),
  });
  return handleResponse<ShoppingSessionResponse>(res);
}

export async function authorizePurchase(
  sessionId: string,
  payload: AuthorizePurchaseRequest
): Promise<AuthorizePurchaseResponse> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(
    `/api/shopping/sessions/${encodeURIComponent(trimmed)}/authorize`,
    {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(payload),
    }
  );
  return handleResponse<AuthorizePurchaseResponse>(res);
}

export async function createPayPalOrder(
  sessionId: string
): Promise<CreateOrderResponse> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(
    `/api/shopping/sessions/${encodeURIComponent(trimmed)}/orders`,
    {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
    }
  );
  return handleResponse<CreateOrderResponse>(res);
}

export async function capturePayPalOrder(
  sessionId: string
): Promise<CaptureOrderResponse> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(
    `/api/shopping/sessions/${encodeURIComponent(trimmed)}/capture`,
    {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
    }
  );
  return handleResponse<CaptureOrderResponse>(res);
}

export async function fetchPayPalOrderStatus(
  sessionId: string
): Promise<PayPalOrderStatusResponse> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(
    `/api/shopping/sessions/${encodeURIComponent(trimmed)}/order-status`
  );
  return handleResponse<PayPalOrderStatusResponse>(res);
}

export async function verifySession(
  sessionId: string
): Promise<VerificationResult> {
  const trimmed = sessionId.trim();
  if (!trimmed) {
    throw new ApiClientError('Session ID cannot be empty', 400);
  }
  const res = await fetch(`/api/sessions/${encodeURIComponent(trimmed)}/verify`);
  return handleResponse<VerificationResult>(res);
}
