import { useState, useCallback, useEffect } from 'react';
import type {
  ShoppingSessionResponse,
  ShoppingSessionDetail,
  PurchaseAuthorization,
  CreateOrderResponse,
  CaptureOrderResponse,
  PayPalOrderStatusResponse,
  DecisionStep,
  VerificationResult,
  AuthorizePurchaseRequest,
} from '../types/api';
import {
  createShoppingSession,
  fetchShoppingSession,
  authorizePurchase as apiAuthorizePurchase,
  createPayPalOrder,
  capturePayPalOrder,
  fetchPayPalOrderStatus,
  fetchSessionEvents,
  verifySession,
  ApiClientError,
} from '../api/client';

const RECENT_SESSIONS_STORAGE_KEY = 'agentaudit_recent_sessions';
const MAX_RECENT_SESSIONS = 10;

export type JourneyPhase =
  | 'idle'
  | 'analyzing'
  | 'analyzed'
  | 'authorizing'
  | 'authorized'
  | 'creating_order'
  | 'awaiting_approval'
  | 'buyer_approved'
  | 'capturing'
  | 'completed'
  | 'rejected'
  | 'error';

function loadRecentSessionIds(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_SESSIONS_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return parsed.filter((id): id is string => typeof id === 'string' && id.trim().length > 0);
    }
  } catch {
    // ignore storage read failure
  }
  return [];
}

function saveRecentSessionId(sessionId: string): string[] {
  try {
    const trimmed = sessionId.trim();
    if (!trimmed) return loadRecentSessionIds();
    const existing = loadRecentSessionIds();
    const updated = [trimmed, ...existing.filter((id) => id !== trimmed)].slice(0, MAX_RECENT_SESSIONS);
    localStorage.setItem(RECENT_SESSIONS_STORAGE_KEY, JSON.stringify(updated));
    return updated;
  } catch {
    return loadRecentSessionIds();
  }
}

export interface UseShoppingJourneyReturn {
  phase: JourneyPhase;
  prompt: string;
  sessionId: string | null;
  sessionResponse: ShoppingSessionResponse | null;
  authorization: PurchaseAuthorization | null;
  order: CreateOrderResponse | null;
  orderStatus: PayPalOrderStatusResponse | null;
  approvalPopupOpened: boolean;
  capture: CaptureOrderResponse | null;
  sessionDetail: ShoppingSessionDetail | null;
  events: DecisionStep[] | null;
  verification: VerificationResult | null;

  isAnalyzing: boolean;
  isAuthorizing: boolean;
  isCreatingOrder: boolean;
  isCheckingOrderStatus: boolean;
  isCapturing: boolean;
  isLoadingAudit: boolean;
  error: string | null;
  recentSessionIds: string[];

  // Action dispatches
  setPrompt: (text: string) => void;
  startShopping: (promptText: string) => Promise<ShoppingSessionResponse>;
  authorizePurchase: (approved: boolean, customCeiling?: number) => Promise<PurchaseAuthorization>;
  createOrder: () => Promise<CreateOrderResponse>;
  checkOrderStatus: (targetSessionId?: string) => Promise<PayPalOrderStatusResponse>;
  openBuyerApproval: () => Window | null;
  captureOrder: () => Promise<CaptureOrderResponse>;
  loadAudit: (targetSessionId?: string) => Promise<void>;
  loadRecentSession: (targetSessionId: string) => Promise<void>;
  resetJourney: () => void;
  clearError: () => void;
  clearRecentSessions: () => void;
}

export function useShoppingJourney(): UseShoppingJourneyReturn {
  const [phase, setPhase] = useState<JourneyPhase>('idle');
  const [prompt, setPrompt] = useState<string>('');
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [sessionResponse, setSessionResponse] = useState<ShoppingSessionResponse | null>(null);
  const [authorization, setAuthorization] = useState<PurchaseAuthorization | null>(null);
  const [order, setOrder] = useState<CreateOrderResponse | null>(null);
  const [orderStatus, setOrderStatus] = useState<PayPalOrderStatusResponse | null>(null);
  const [approvalPopupOpened, setApprovalPopupOpened] = useState<boolean>(false);
  const [capture, setCapture] = useState<CaptureOrderResponse | null>(null);
  const [sessionDetail, setSessionDetail] = useState<ShoppingSessionDetail | null>(null);
  const [events, setEvents] = useState<DecisionStep[] | null>(null);
  const [verification, setVerification] = useState<VerificationResult | null>(null);

  const [isAnalyzing, setIsAnalyzing] = useState<boolean>(false);
  const [isAuthorizing, setIsAuthorizing] = useState<boolean>(false);
  const [isCreatingOrder, setIsCreatingOrder] = useState<boolean>(false);
  const [isCheckingOrderStatus, setIsCheckingOrderStatus] = useState<boolean>(false);
  const [isCapturing, setIsCapturing] = useState<boolean>(false);
  const [isLoadingAudit, setIsLoadingAudit] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [recentSessionIds, setRecentSessionIds] = useState<string[]>([]);

  // Load recent sessions on initial mount
  useEffect(() => {
    setRecentSessionIds(loadRecentSessionIds());
  }, []);

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  const clearRecentSessions = useCallback(() => {
    try {
      localStorage.removeItem(RECENT_SESSIONS_STORAGE_KEY);
    } catch {
      // ignore
    }
    setRecentSessionIds([]);
  }, []);

  const resetJourney = useCallback(() => {
    setPhase('idle');
    setPrompt('');
    setSessionId(null);
    setSessionResponse(null);
    setAuthorization(null);
    setOrder(null);
    setOrderStatus(null);
    setApprovalPopupOpened(false);
    setCapture(null);
    setSessionDetail(null);
    setEvents(null);
    setVerification(null);
    setError(null);
    setIsAnalyzing(false);
    setIsAuthorizing(false);
    setIsCreatingOrder(false);
    setIsCheckingOrderStatus(false);
    setIsCapturing(false);
    setIsLoadingAudit(false);
  }, []);

  /**
   * Start a new shopping journey given a natural language prompt.
   * Resets any existing session state.
   */
  const startShopping = useCallback(
    async (promptText: string): Promise<ShoppingSessionResponse> => {
      const trimmed = promptText.trim();
      if (!trimmed) {
        const msg = 'Shopping prompt cannot be empty';
        setError(msg);
        throw new Error(msg);
      }

      if (isAnalyzing) {
        throw new Error('Shopping analysis is already in progress');
      }

      // Reset downstream state
      setError(null);
      setPrompt(trimmed);
      setSessionId(null);
      setSessionResponse(null);
      setAuthorization(null);
      setOrder(null);
      setOrderStatus(null);
      setApprovalPopupOpened(false);
      setCapture(null);
      setSessionDetail(null);
      setEvents(null);
      setVerification(null);
      setIsAnalyzing(true);
      setPhase('analyzing');

      try {
        const response = await createShoppingSession(trimmed);
        setSessionResponse(response);
        setSessionId(response.session_id);
        setRecentSessionIds(saveRecentSessionId(response.session_id));
        setPhase('analyzed');
        return response;
      } catch (err: unknown) {
        const message =
          err instanceof ApiClientError
            ? err.message
            : err instanceof Error
            ? err.message
            : 'Failed to analyze shopping prompt';
        setError(message);
        setPhase('error');
        throw err;
      } finally {
        setIsAnalyzing(false);
      }
    },
    [isAnalyzing]
  );

  /**
   * Authorize or reject purchasing the selected product.
   */
  const authorizePurchase = useCallback(
    async (approved: boolean, customCeiling?: number): Promise<PurchaseAuthorization> => {
      if (!sessionId) {
        const msg = 'Cannot authorize purchase: no active shopping session';
        setError(msg);
        throw new Error(msg);
      }

      if (!sessionResponse?.selected_product) {
        const msg = 'Cannot authorize purchase: no selected product in active session';
        setError(msg);
        throw new Error(msg);
      }

      if (isAuthorizing) {
        throw new Error('Purchase authorization is already in progress');
      }

      const product = sessionResponse.selected_product;
      const ceiling = customCeiling !== undefined ? customCeiling : product.price;

      if (approved && ceiling < product.price) {
        const msg = `Authorization ceiling (${ceiling}) cannot be less than product price (${product.price})`;
        setError(msg);
        throw new Error(msg);
      }

      setError(null);
      setIsAuthorizing(true);
      setPhase('authorizing');

      const payload: AuthorizePurchaseRequest = {
        selected_product_id: product.product_id,
        maximum_authorized_amount: ceiling,
        currency: product.currency || 'USD',
        approved,
      };

      if (!approved) {
        setPhase('rejected');
        setError('Purchase authorization was explicitly declined by customer. No payment or PayPal order will be created.');
        try {
          await apiAuthorizePurchase(sessionId, payload);
        } catch {
          // Expected backend decline error
        }
        const declineAuth: PurchaseAuthorization = {
          session_id: sessionId,
          selected_product_id: product.product_id,
          maximum_authorized_amount: ceiling,
          currency: product.currency || 'USD',
          authorization_status: 'REJECTED',
          authorization_timestamp: new Date().toISOString(),
          authorization_method: 'CUSTOMER_EXPLICIT_DECLINE',
        };
        setAuthorization(declineAuth);
        return declineAuth;
      }

      try {
        const response = await apiAuthorizePurchase(sessionId, payload);
        setAuthorization(response.authorization);
        setPhase('authorized');
        return response.authorization;
      } catch (err: unknown) {
        const message =
          err instanceof ApiClientError
            ? err.message
            : err instanceof Error
            ? err.message
            : 'Failed to authorize purchase';
        setError(message);
        setPhase('error');
        throw err;
      } finally {
        setIsAuthorizing(false);
      }
    },
    [sessionId, sessionResponse, isAuthorizing]
  );

  /**
   * Create a PayPal Orders v2 Sandbox order for the authorized purchase.
   */
  const createOrder = useCallback(async (): Promise<CreateOrderResponse> => {
    if (!sessionId) {
      const msg = 'Cannot create PayPal order: no active shopping session';
      setError(msg);
      throw new Error(msg);
    }

    if (!authorization) {
      const msg = 'Cannot create PayPal order: purchase has not been authorized';
      setError(msg);
      throw new Error(msg);
    }

    const authStatus = String(authorization.authorization_status).toUpperCase();
    if (authStatus !== 'APPROVED') {
      const msg = 'Cannot create PayPal order: purchase authorization was rejected';
      setError(msg);
      throw new Error(msg);
    }

    if (isCreatingOrder) {
      throw new Error('PayPal order creation is already in progress');
    }

    setError(null);
    setIsCreatingOrder(true);
    setPhase('creating_order');

    try {
      const response = await createPayPalOrder(sessionId);
      setOrder(response);
      setPhase('awaiting_approval');
      return response;
    } catch (err: unknown) {
      const message =
        err instanceof ApiClientError
          ? err.message
          : err instanceof Error
          ? err.message
          : 'Failed to create PayPal order';
      setError(message);
      setPhase('error');
      throw err;
    } finally {
      setIsCreatingOrder(false);
    }
  }, [sessionId, authorization, isCreatingOrder]);

  /**
   * Open the PayPal Sandbox buyer approval URL in a dedicated popup window.
   * Note: Closing the popup is NOT treated as buyer approval. The buyer must
   * log into PayPal Sandbox and approve the order before capture can succeed.
   */
  const openBuyerApproval = useCallback((): Window | null => {
    if (!order?.approval_url) {
      setError('Cannot open PayPal approval: approval URL is not available');
      return null;
    }

    setError(null);
    const popup = window.open(
      order.approval_url,
      'AgentAudit_PayPal_Sandbox_Checkout',
      'width=650,height=750,location=yes,status=yes,scrollbars=yes'
    );

    if (popup) {
      setApprovalPopupOpened(true);
      popup.focus();
    } else {
      setError('Popup blocked by browser. Please allow popups to complete PayPal approval.');
    }

    return popup;
  }, [order]);

  /**
   * Authoritatively query PayPal Orders v2 for live buyer approval status.
   */
  const checkOrderStatus = useCallback(
    async (targetSessionId?: string): Promise<PayPalOrderStatusResponse> => {
      const idToCheck = (targetSessionId || sessionId || '').trim();
      if (!idToCheck) {
        const msg = 'Cannot check order status: no active session ID';
        setError(msg);
        throw new Error(msg);
      }

      if (isCheckingOrderStatus) {
        throw new Error('Order status check is already in progress');
      }

      setError(null);
      setIsCheckingOrderStatus(true);

      try {
        const res = await fetchPayPalOrderStatus(idToCheck);
        setOrderStatus(res);
        if (res.is_approved) {
          setPhase('buyer_approved');
        } else if (res.is_completed) {
          setPhase('completed');
        } else if (res.status === 'CREATED' || res.status === 'PAYER_ACTION_REQUIRED') {
          setPhase('awaiting_approval');
        }
        return res;
      } catch (err: unknown) {
        const message =
          err instanceof ApiClientError
            ? err.message
            : err instanceof Error
            ? err.message
            : 'Failed to verify PayPal order status';
        setError(message);
        throw err;
      } finally {
        setIsCheckingOrderStatus(false);
      }
    },
    [sessionId, isCheckingOrderStatus]
  );

  /**
   * Capture the PayPal Sandbox order and finalize the cryptographic ledger.
   */
  const captureOrder = useCallback(async (): Promise<CaptureOrderResponse> => {
    if (!sessionId) {
      const msg = 'Cannot capture order: no active shopping session';
      setError(msg);
      throw new Error(msg);
    }

    if (!order?.order_id) {
      const msg = 'Cannot capture order: no PayPal order has been created';
      setError(msg);
      throw new Error(msg);
    }

    // Require authoritative buyer approval confirmation before capture
    if (orderStatus && !orderStatus.is_approved && !orderStatus.is_completed) {
      const msg = `Cannot capture payment: PayPal order status is '${orderStatus.status}'. Buyer approval has not been confirmed.`;
      setError(msg);
      throw new Error(msg);
    }

    if (isCapturing) {
      throw new Error('Order capture is already in progress');
    }

    setError(null);
    setIsCapturing(true);
    setPhase('capturing');

    try {
      const response = await capturePayPalOrder(sessionId);
      setCapture(response);
      setPhase('completed');
      setOrderStatus((prev) =>
        prev ? { ...prev, status: 'COMPLETED', is_approved: true, is_completed: true } : null
      );

      // Automatically load the finalized audit trail and verification
      try {
        const [detail, eventList, verifyRes] = await Promise.all([
          fetchShoppingSession(sessionId),
          fetchSessionEvents(sessionId),
          verifySession(sessionId),
        ]);
        setSessionDetail(detail);
        setEvents(eventList);
        setVerification(verifyRes);
      } catch {
        // Non-fatal: capture succeeded even if audit load had a transient issue
      }

      return response;
    } catch (err: unknown) {
      const message =
        err instanceof ApiClientError
          ? err.message
          : err instanceof Error
          ? err.message
          : 'Failed to capture PayPal order';
      setError(message);
      // Revert phase to buyer_approved if was already approved, otherwise awaiting_approval
      setPhase(orderStatus?.is_approved ? 'buyer_approved' : 'awaiting_approval');
      throw err;
    } finally {
      setIsCapturing(false);
    }
  }, [sessionId, order, orderStatus, isCapturing]);

  /**
   * Load the audit trail, decision steps, and verification for a session.
   */
  const loadAudit = useCallback(
    async (targetSessionId?: string): Promise<void> => {
      const idToLoad = (targetSessionId || sessionId || '').trim();
      if (!idToLoad) {
        const msg = 'Cannot load audit: no session ID provided';
        setError(msg);
        throw new Error(msg);
      }

      if (isLoadingAudit) {
        throw new Error('Audit load is already in progress');
      }

      setError(null);
      setIsLoadingAudit(true);

      try {
        const [detail, eventList, verifyRes] = await Promise.all([
          fetchShoppingSession(idToLoad),
          fetchSessionEvents(idToLoad),
          verifySession(idToLoad),
        ]);
        setSessionDetail(detail);
        setEvents(eventList);
        setVerification(verifyRes);
      } catch (err: unknown) {
        const message =
          err instanceof ApiClientError
            ? err.message
            : err instanceof Error
            ? err.message
            : 'Failed to load session audit';
        setError(message);
        throw err;
      } finally {
        setIsLoadingAudit(false);
      }
    },
    [sessionId, isLoadingAudit]
  );

  /**
   * Restore/inspect a historical session from its session ID.
   */
  const loadRecentSession = useCallback(
    async (targetSessionId: string): Promise<void> => {
      const trimmed = targetSessionId.trim();
      if (!trimmed) {
        const msg = 'Session ID cannot be empty';
        setError(msg);
        throw new Error(msg);
      }

      setError(null);
      setIsLoadingAudit(true);

      try {
        const [detail, eventList, verifyRes] = await Promise.all([
          fetchShoppingSession(trimmed),
          fetchSessionEvents(trimmed),
          verifySession(trimmed),
        ]);

        setSessionId(detail.session_id);
        setPrompt(detail.initial_prompt);
        setSessionDetail(detail);
        setEvents(eventList);
        setVerification(verifyRes);
        setAuthorization(detail.authorization || null);

        if (detail.requirements && detail.selected_product) {
          setSessionResponse({
            success: true,
            session_id: detail.session_id,
            prompt: detail.initial_prompt,
            requirements: detail.requirements,
            candidates_found: eventList.length > 0 ? 3 : 0,
            rejected_candidates: [],
            selected_product: detail.selected_product,
            selection_rationale: null,
            current_sequence_index: detail.event_count,
            latest_hash: detail.root_hash || '',
          });
        }

        if (detail.paypal_order_id) {
          setOrder({
            success: true,
            session_id: detail.session_id,
            order_id: detail.paypal_order_id,
            status: detail.is_finalized ? 'COMPLETED' : 'APPROVED',
            approval_url: null,
          });
        }

        if (detail.is_finalized) {
          setPhase('completed');
          setCapture({
            success: true,
            session_id: detail.session_id,
            order_id: detail.paypal_order_id || '',
            capture_id: null,
            status: 'COMPLETED',
            final_root_hash: detail.root_hash || '',
          });
        } else if (detail.authorization) {
          setPhase('authorized');
        } else if (detail.selected_product) {
          setPhase('analyzed');
        } else {
          setPhase('idle');
        }

        setRecentSessionIds(saveRecentSessionId(trimmed));
      } catch (err: unknown) {
        const message =
          err instanceof ApiClientError
            ? err.message
            : err instanceof Error
            ? err.message
            : `Failed to load session ${trimmed}`;
        setError(message);
        throw err;
      } finally {
        setIsLoadingAudit(false);
      }
    },
    []
  );

  return {
    phase,
    prompt,
    sessionId,
    sessionResponse,
    authorization,
    order,
    orderStatus,
    approvalPopupOpened,
    capture,
    sessionDetail,
    events,
    verification,

    isAnalyzing,
    isAuthorizing,
    isCreatingOrder,
    isCheckingOrderStatus,
    isCapturing,
    isLoadingAudit,
    error,
    recentSessionIds,

    startShopping,
    setPrompt,
    authorizePurchase,
    createOrder,
    checkOrderStatus,
    openBuyerApproval,
    captureOrder,
    loadAudit,
    loadRecentSession,
    resetJourney,
    clearError,
    clearRecentSessions,
  };
}
