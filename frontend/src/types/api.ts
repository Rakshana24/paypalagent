export interface HealthResponse {
  success: boolean;
  message: string;
}

export interface AuthCheckResponse {
  success: boolean;
  message: string;
  token_type: string;
  expires_in: number;
}

export type EpistemicCategory =
  | 'VERIFIED_FACT'
  | 'RECORDED_PAYPAL_DATA'
  | 'DETERMINISTIC_INFERENCE'
  | 'UNAVAILABLE_EVIDENCE';

export type SettlementProvenance =
  | 'LIVE_PAYPAL_API'
  | 'REPLAYED_FROM_LEDGER_PAYLOAD';

export type UnavailableEvidenceType =
  | 'PHYSICAL_SHIPPING_TRACKING_AND_PROOF_OF_DELIVERY'
  | 'BUYER_PHYSICAL_IDENTITY_PROOF'
  | 'BUYER_MERCHANT_COMMUNICATIONS'
  | 'CARDHOLDER_BANK_COMMUNICATIONS'
  | 'RETURN_MERCHANDISE_RECEIPT'
  | 'REFUND_OFFER_DOCUMENTATION';

export interface EvidenceSourceReference {
  session_id?: string;
  dispute_id?: string;
  sequence_index?: number;
  action_type?: string;
  step_hash?: string;
  paypal_record_id?: string;
  webhook_transmission_id?: string;
  timestamp?: string;
}

export type CorrelationStatus =
  | 'MATCHED'
  | 'MANUAL_REVIEW_REQUIRED'
  | 'NOT_FOUND';

export type MatchedBy = 'ORDER_ID' | 'CAPTURE_ID' | 'SESSION_ID_EXPLICIT';

export interface CorrelationSummary {
  category: EpistemicCategory;
  status: CorrelationStatus;
  evaluator: string;
  matched_by?: MatchedBy;
  session_id?: string;
  explanation: string;
  source?: EvidenceSourceReference;
}

export interface DisputeSummaryFact {
  category: EpistemicCategory;
  dispute_id: string;
  status: string;
  dispute_lifecycle_stage?: string;
  dispute_channel?: string;
  reason?: string;
  dispute_amount_value?: string;
  dispute_amount_currency?: string;
  seller_transaction_id?: string;
  paypal_order_id?: string;
  dispute_create_time?: string;
  dispute_update_time?: string;
  source?: EvidenceSourceReference;
}

export type GenesisAnchorType = 'GENESIS_HASH' | 'PURCHASE_ROOT_HASH';

export interface CryptographicProofSummary {
  category: EpistemicCategory;
  purchase_session_id?: string;
  purchase_root_hash?: string;
  purchase_root_hash_source?: EvidenceSourceReference;
  purchase_chain_valid: boolean;
  purchase_total_steps: number;
  dispute_root_hash?: string;
  dispute_root_hash_source?: EvidenceSourceReference;
  dispute_chain_valid: boolean;
  dispute_total_steps: number;
  genesis_anchor_type: GenesisAnchorType;
  genesis_anchor_hash: string;
  dual_chain_integrity_verified: boolean;
  integrity_scope_statement: string;
}

export interface AgentStatementFact {
  statement: string;
  disclaimer: string;
}

export interface PromptAuditFact {
  prompt: string;
  source: EvidenceSourceReference;
}

export interface RequirementsAuditFact {
  category: string;
  maximum_budget: number;
  preferred_brand?: string;
  minimum_battery_hours?: number;
  required_features: string[];
  intended_use?: string;
  source: EvidenceSourceReference;
}

export interface CatalogSearchAuditFact {
  candidates_evaluated_count: number;
  source: EvidenceSourceReference;
}

export interface RejectedProductAuditFact {
  product_id: string;
  product_name: string;
  price: number;
  currency: string;
  rejection_rationale: AgentStatementFact;
  source: EvidenceSourceReference;
}

export interface SelectedProductAuditFact {
  product_id: string;
  product_name: string;
  brand: string;
  category: string;
  price: number;
  currency: string;
  selection_rationale: AgentStatementFact;
  source: EvidenceSourceReference;
}

export interface AuthorizationAuditFact {
  selected_product_id: string;
  maximum_authorized_amount: number;
  user_confirmed: boolean;
  authorized_at: string;
  identity_disclaimer: string;
  source: EvidenceSourceReference;
}

export interface AgentAuditTrailSummary {
  category: EpistemicCategory;
  initial_prompt: PromptAuditFact;
  requirements?: RequirementsAuditFact;
  catalog_search?: CatalogSearchAuditFact;
  rejected_candidates: RejectedProductAuditFact[];
  selected_product?: SelectedProductAuditFact;
  purchase_authorization?: AuthorizationAuditFact;
}

export interface RecordedPaypalSettlementFact {
  category: EpistemicCategory;
  provenance: SettlementProvenance;
  source: EvidenceSourceReference;
  paypal_order_id?: string;
  order_status?: string;
  order_currency?: string;
  order_amount?: string;
  paypal_capture_id?: string;
  capture_status?: string;
  capture_timestamp?: string;
}

export interface VerifiedLedgerSettlementFact {
  category: EpistemicCategory;
  order_created_step?: EvidenceSourceReference;
  payment_captured_step?: EvidenceSourceReference;
}

export interface TransactionProof {
  paypal_settlement: RecordedPaypalSettlementFact;
  ledger_settlement: VerifiedLedgerSettlementFact;
}

export interface RebuttalPoint {
  category: EpistemicCategory;
  claim: string;
  explanation: string;
  source_references: EvidenceSourceReference[];
}

export interface RebuttalSummary {
  category: EpistemicCategory;
  dispute_reason?: string;
  recommended_action: string;
  primary_arguments: RebuttalPoint[];
  reason_specific_analysis: string;
}

export interface MissingEvidenceItem {
  evidence_type: UnavailableEvidenceType;
  reason_unavailable: string;
  adjudication_impact: string;
}

export interface MissingEvidenceDisclaimer {
  category: EpistemicCategory;
  missing_items: MissingEvidenceItem[];
  general_disclaimer: string;
}

export interface DisputeEvidenceBrief {
  brief_id: string;
  schema_version: string;
  dispute_id: string;
  generated_at: string;
  correlation_summary: CorrelationSummary;
  dispute_summary: DisputeSummaryFact;
  cryptographic_proof: CryptographicProofSummary;
  agent_audit_trail?: AgentAuditTrailSummary;
  transaction_proof?: TransactionProof;
  rebuttal_summary: RebuttalSummary;
  missing_evidence: MissingEvidenceDisclaimer;
}

export interface ApiErrorResponse {
  success: false;
  message: string;
}

export interface ShoppingRequirements {
  category: string;
  maximum_budget: number;
  currency: string;
  required_features: string[];
  minimum_battery_hours?: number;
  preferred_brand?: string | null;
}

export interface ProductSpecifications {
  battery_life_hours?: number;
  noise_cancelling: boolean;
  anc_type?: string;
  wireless: boolean;
  bluetooth_version?: string;
  weight_grams?: number;
  microphone: boolean;
  water_resistant: boolean;
}

export interface Product {
  product_id: string;
  name: string;
  brand: string;
  category: string;
  price: number;
  currency: string;
  rating: number;
  available_stock: number;
  description: string;
  specifications: ProductSpecifications;
  features: string[];
}

export interface PurchaseAuthorization {
  session_id: string;
  selected_product_id: string;
  maximum_authorized_amount: number;
  currency: string;
  authorization_status: string;
  authorization_timestamp: string;
  authorization_method: string;
}

export interface VerificationResult {
  session_id: string;
  status: string;
  total_steps: number;
  verified_steps: number;
  calculated_root_hash: string;
  stored_root_hash?: string | null;
  first_invalid_sequence_index?: number | null;
  error_code?: string | null;
  explanation: string;
}

export interface ShoppingSessionDetail {
  session_id: string;
  initial_prompt: string;
  requirements?: ShoppingRequirements | null;
  selected_product?: Product | null;
  authorization?: PurchaseAuthorization | null;
  paypal_order_id?: string | null;
  is_finalized: boolean;
  root_hash?: string | null;
  event_count: number;
  verification: VerificationResult;
}

export interface DecisionStep {
  id: string;
  session_id: string;
  sequence_index: number;
  action_type: string;
  payload: string;
  previous_hash: string;
  current_hash: string;
  created_at: string;
}
